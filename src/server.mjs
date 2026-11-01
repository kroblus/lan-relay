import http from 'node:http';
import https from 'node:https';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { open, stat, rename, unlink } from 'node:fs/promises';
import { createReadStream } from 'node:fs';
import { pipeline } from 'node:stream/promises';
import { createHash, randomInt } from 'node:crypto';
import { RelayStore, VERSION, now, id, secret, digest } from './store.mjs';
import { inspectApk as inspectManifestApk } from './apk.mjs';
import { HttpError, fail, safeEqual, shortText, deviceName, contentText, fileName, mimeType,
  readBody, readJson, disposition, previewMime, findAnalyzer, inspectApk as inspectSdkApk, mdLabel, fencedText } from './helpers.mjs';

const ROOT = fileURLToPath(new URL('../', import.meta.url));
const MUTATING = new Set(['POST', 'PATCH', 'PUT', 'DELETE']);
const TEST_STATUSES = new Set(['not-tested', 'user-confirmed-installed', 'issue-found', 'user-confirmed-passed']);
const STATIC_TYPES = { '.html': 'text/html; charset=utf-8', '.css': 'text/css; charset=utf-8', '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8', '.json': 'application/json; charset=utf-8', '.svg': 'image/svg+xml', '.png': 'image/png',
  '.ico': 'image/x-icon', '.webmanifest': 'application/manifest+json', '.woff2': 'font/woff2' };

function numberOption(value, fallback, label, minimum = 1) {
  const result = value ?? fallback;
  if (!Number.isSafeInteger(result) || result < minimum) throw new TypeError(`${label} must be a safe integer >= ${minimum}`);
  return result;
}

function publicItem(item) {
  if (!item) return null;
  const { _storageId, _previewMime, ...visible } = item;
  return visible;
}

function cookieToken(req) {
  const match = /(?:^|;\s*)relay_session=([^;]*)/.exec(req.headers.cookie || '');
  if (!match) return null;
  try { return decodeURIComponent(match[1]); } catch { return null; }
}

function json(res, status, value) {
  const body = JSON.stringify(value);
  res.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8', 'Content-Length': Buffer.byteLength(body) });
  res.end(body);
}

function localAddresses() {
  const result = new Set(['127.0.0.1', '::1', 'localhost']);
  for (const entries of Object.values(os.networkInterfaces())) {
    for (const entry of entries || []) result.add(entry.address.split('%')[0]);
  }
  return result;
}

export async function createRelayServer(options = {}) {
  const maxFileBytes = numberOption(options.maxFileBytes, 2 * 1024 ** 3, 'maxFileBytes', 0);
  const maxStorageBytes = numberOption(options.maxStorageBytes, 20 * 1024 ** 3, 'maxStorageBytes', 0);
  const chunkSize = numberOption(options.chunkSize, 2 * 1024 ** 2, 'chunkSize');
  if (chunkSize > 16 * 1024 ** 2) throw new TypeError('chunkSize must not exceed 16 MiB');
  const publicDir = path.resolve(options.publicDir || path.join(ROOT, 'public'));
  const allowedOrigins = new Set();
  const previewHosts = new Set();
  for (const candidate of options.allowedOrigins || []) {
    const parsed = new URL(candidate);
    if (!['http:', 'https:'].includes(parsed.protocol) || parsed.origin !== candidate || parsed.username || parsed.password) {
      throw new TypeError('allowedOrigins must contain exact http(s) origins, without paths or wildcards');
    }
    allowedOrigins.add(parsed.origin);
    previewHosts.add(parsed.host);
  }
  const secure = Boolean(options.tls);
  if (secure && (!options.tls.key || !options.tls.cert)) throw new TypeError('TLS requires both key and cert');
  const analyzer = await findAnalyzer(options.apkAnalyzerPath);
  const store = await RelayStore.open(options.dataDir || path.join(ROOT, '.lan-relay-data'));
  const uploadLocks = new Set();
  const streams = new Set();
  const requests = new Set();
  const rates = new Map();
  let closing = false;
  let closedPromise;

  const connectionUrls = () => {
    const address = server.address();
    const port = typeof address === 'object' && address ? address.port : 4318;
    const addresses = [];
    for (const entries of Object.values(os.networkInterfaces())) {
      for (const entry of entries || []) if (!entry.internal && entry.family === 'IPv4') addresses.push(entry.address);
    }
    addresses.push('127.0.0.1');
    return [...new Set(addresses)].map(host => `${secure ? 'https' : 'http'}://${host}:${port}`);
  };

  function validateRequest(req) {
    if (!req.headers.host || typeof req.headers.host !== 'string' || /[\s@\\/#?]/.test(req.headers.host)) {
      fail(403, 'INVALID_HOST', '不允许此访问地址。');
    }
    let requestOrigin;
    try { requestOrigin = new URL(`${secure ? 'https' : 'http'}://${req.headers.host}`); }
    catch { fail(403, 'INVALID_HOST', '不允许此访问地址。'); }
    const host = requestOrigin.hostname.replace(/^\[|\]$/g, '');
    if (!localAddresses().has(host) && !previewHosts.has(requestOrigin.host)) {
      fail(403, 'INVALID_HOST', '此主机名未获允许，请使用电脑显示的局域网地址。');
    }
    const origin = req.headers.origin;
    if (origin && origin !== requestOrigin.origin && !allowedOrigins.has(origin)) {
      fail(403, 'INVALID_ORIGIN', '请求来源未获允许。');
    }
    if (!req.url?.startsWith('/') || req.url.startsWith('//')) fail(400, 'INVALID_URL', '请求地址无效。');
    return new URL(req.url, requestOrigin);
  }

  function authenticate(req, ownerOnly = false, mutation = false) {
    const session = store.session(cookieToken(req));
    if (!session) fail(401, 'UNAUTHENTICATED', '请先连接或配对此设备。');
    if (ownerOnly && session.device.role !== 'owner') fail(403, 'OWNER_REQUIRED', '此操作需要电脑管理者权限。');
    if (mutation && !safeEqual(req.headers['x-csrf-token'], session.csrfToken)) fail(403, 'CSRF_REQUIRED', '安全校验已过期，请刷新页面后重试。');
    return session;
  }

  function rateLimit(req, group, maximum, windowMs = 5 * 60_000) {
    const time = Date.now();
    const key = `${group}:${req.socket.remoteAddress || 'unknown'}`;
    let rate = rates.get(key);
    if (!rate || rate.end <= time) { rate = { count: 0, end: time + windowMs }; rates.set(key, rate); }
    rate.count++;
    if (rates.size > 1000) for (const [entry, value] of rates) if (value.end <= time) rates.delete(entry);
    if (rate.count > maximum) fail(429, 'RATE_LIMITED', '尝试过于频繁，请稍后再试。', { retryAfter: Math.ceil((rate.end - time) / 1000) });
  }

  function setSession(res, session) {
    res.setHeader('Set-Cookie', `relay_session=${session.token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=7776000${secure ? '; Secure' : ''}`);
  }

  function broadcast(type) {
    const message = `event: change\ndata: ${JSON.stringify({ type })}\n\n`;
    for (const entry of streams) {
      if (entry.res.destroyed || entry.res.writableEnded) continue;
      // An inactive browser must not make server memory grow without bounds.
      if (entry.res.writableLength > 64 * 1024) entry.res.end();
      else entry.res.write(message);
    }
  }

  function buildReference(value) {
    if (value === undefined || value === null || value === '') return undefined;
    const buildId = shortText(value, '构建标识', 80);
    const item = store.getItem(buildId);
    if (!item || item.kind !== 'file' || !item.isApk) fail(400, 'INVALID_BUILD', '关联构建必须是已传完的 APK。');
    return buildId;
  }

  function uploadAccess(uploadId, session, write = false) {
    const row = store.db.prepare('SELECT * FROM uploads WHERE id=?').get(uploadId);
    if (!row) fail(404, 'UPLOAD_NOT_FOUND', '未找到此传输。');
    if (row.device_id !== session.device.id && (write || session.device.role !== 'owner')) fail(403, 'UPLOAD_FORBIDDEN', '无法操作其他设备的传输。');
    return row;
  }

  async function lockedUpload(uploadId, fn) {
    if (uploadLocks.has(uploadId)) fail(409, 'UPLOAD_BUSY', '此文件正在处理，请稍后重试。', { offset: store.getUpload(uploadId, chunkSize)?.offset });
    uploadLocks.add(uploadId);
    try { return await fn(); } finally { uploadLocks.delete(uploadId); }
  }

  async function serveFile(req, res, fileId, preview) {
    const item = store.getItem(fileId);
    if (!item || item.kind !== 'file' || !item._storageId) fail(404, 'FILE_NOT_FOUND', '未找到文件。');
    if (preview && !item._previewMime) fail(415, 'PREVIEW_UNAVAILABLE', '此文件不支持安全预览，请下载后查看。');
    const filePath = store.filePath(item._storageId);
    let info;
    try { info = await stat(filePath); }
    catch (error) { if (error.code === 'ENOENT') fail(410, 'FILE_MISSING', '文件已从电脑移除。'); throw error; }
    if (!info.isFile() || info.size !== item.size) fail(409, 'FILE_CHANGED', '保存的文件与记录不一致，请重新传输。');
    const etag = `"${item.sha256}"`;
    const headers = {
      'Content-Type': preview ? item._previewMime : 'application/octet-stream',
      'Content-Disposition': disposition(item.name, preview),
      'Accept-Ranges': 'bytes', ETag: etag,
      'Content-Security-Policy': "default-src 'none'; sandbox",
      'Cross-Origin-Resource-Policy': 'same-origin'
    };
    let start = 0, end = info.size - 1, status = 200;
    const range = req.headers.range;
    if (range && (!req.headers['if-range'] || req.headers['if-range'] === etag)) {
      const match = /^bytes=(\d*)-(\d*)$/.exec(range);
      let valid = Boolean(match && (match[1] || match[2]) && info.size > 0);
      if (valid && match[1]) {
        start = Number(match[1]); end = match[2] ? Number(match[2]) : info.size - 1;
        valid = Number.isSafeInteger(start) && Number.isSafeInteger(end) && start < info.size && start <= end;
        end = Math.min(end, info.size - 1);
      } else if (valid) {
        const suffix = Number(match[2]);
        valid = Number.isSafeInteger(suffix) && suffix > 0;
        start = Math.max(0, info.size - suffix);
      }
      if (!valid) {
        res.setHeader('Content-Range', `bytes */${info.size}`);
        fail(416, 'INVALID_RANGE', '请求的文件范围无效。');
      }
      status = 206;
      headers['Content-Range'] = `bytes ${start}-${end}/${info.size}`;
    }
    headers['Content-Length'] = Math.max(0, end - start + 1);
    res.writeHead(status, headers);
    if (req.method === 'HEAD' || info.size === 0) { res.end(); return; }
    await pipeline(createReadStream(filePath, { start, end }), res);
  }

  async function serveStatic(req, res, url) {
    if (!['GET', 'HEAD'].includes(req.method)) fail(405, 'METHOD_NOT_ALLOWED', '此请求方法不受支持。');
    let relative;
    try { relative = decodeURIComponent(url.pathname); } catch { fail(400, 'INVALID_URL', '请求地址无效。'); }
    if (relative === '/') relative = '/index.html';
    if (relative.includes('\0') || relative.includes('\\') || relative.split('/').some(segment => segment.startsWith('.'))) fail(404, 'NOT_FOUND', '未找到页面。');
    const filePath = path.resolve(publicDir, `.${relative}`);
    if (!filePath.startsWith(`${publicDir}${path.sep}`)) fail(404, 'NOT_FOUND', '未找到页面。');
    let info;
    try { info = await stat(filePath); } catch (error) { if (error.code === 'ENOENT') fail(404, 'NOT_FOUND', '未找到页面。'); throw error; }
    if (!info.isFile()) fail(404, 'NOT_FOUND', '未找到页面。');
    res.setHeader('Content-Security-Policy', "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' blob: data:; media-src 'self' blob:; connect-src 'self'; worker-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'");
    res.writeHead(200, { 'Content-Type': STATIC_TYPES[path.extname(filePath)] || 'application/octet-stream', 'Content-Length': info.size });
    if (req.method === 'HEAD') res.end();
    else await pipeline(createReadStream(filePath), res);
  }

  async function route(req, res) {
    res.setHeader('X-Content-Type-Options', 'nosniff');
    res.setHeader('Referrer-Policy', 'no-referrer');
    res.setHeader('X-Frame-Options', 'DENY');
    res.setHeader('Cache-Control', 'no-store');
    if (closing) fail(503, 'SHUTTING_DOWN', '电脑服务正在关闭，请稍后重新连接。');
    const url = validateRequest(req);
    const pathname = url.pathname;
    if (!pathname.startsWith('/api/')) return serveStatic(req, res, url);
    const method = req.method;

    if (pathname === '/api/health' && method === 'GET') return json(res, 200, { ok: true, version: VERSION });
    if (pathname === '/api/session' && method === 'GET') {
      const session = store.session(cookieToken(req));
      return json(res, 200, session ? { authenticated: true, device: session.device, csrfToken: session.csrfToken } : { authenticated: false });
    }
    if (pathname === '/api/session/owner' && method === 'POST') {
      rateLimit(req, 'owner', 20);
      const data = await readJson(req);
      if (!safeEqual(data.secret, store.ownerSecret)) fail(401, 'INVALID_OWNER_SECRET', '电脑连接密钥不正确。');
      const name = data.name === undefined ? undefined : deviceName(data.name);
      const session = store.transaction(() => store.createSession(store.owner(name)));
      setSession(res, session);
      broadcast('devices');
      return json(res, 200, { device: session.device, csrfToken: session.csrfToken });
    }
    if (pathname === '/api/pair/requests' && method === 'POST') {
      rateLimit(req, 'pair', 30);
      const data = await readJson(req);
      const name = deviceName(data.name);
      if (Boolean(data.code) === Boolean(data.token)) fail(400, 'INVALID_PAIRING', '请输入一个配对码，或使用扫码连接。');
      const value = data.code || data.token;
      if (typeof value !== 'string' || (data.code ? !/^\d{6}$/.test(value) : !/^[A-Za-z0-9_-]{43}$/.test(value))) fail(400, 'INVALID_PAIRING', '配对信息无效或已过期。');
      const column = data.code ? 'code_hash' : 'token_hash';
      const claimToken = secret();
      const requestId = id();
      store.transaction(() => {
        const pairing = store.db.prepare(`SELECT * FROM pairings WHERE ${column}=? AND used_at IS NULL AND expires_at>?`).get(digest(value), now());
        if (!pairing) fail(400, 'INVALID_PAIRING', '配对信息无效、已使用或已过期。');
        store.db.prepare('UPDATE pairings SET used_at=? WHERE id=?').run(now(), pairing.id);
        store.db.prepare("INSERT INTO pair_requests(id,name,claim_hash,status,created_at,expires_at) VALUES(?,?,?,'pending',?,?)")
          .run(requestId, name, digest(claimToken), now(), new Date(Date.now() + 5 * 60_000).toISOString());
      });
      broadcast('pairing');
      return json(res, 201, { id: requestId, claimToken, status: 'pending' });
    }
    if (pathname === '/api/pair/claim' && method === 'POST') {
      rateLimit(req, 'claim', 240);
      const data = await readJson(req);
      const requestId = shortText(data.id, '配对请求', 80);
      if (typeof data.claimToken !== 'string' || data.claimToken.length > 256) fail(400, 'INVALID_CLAIM', '配对凭据无效。');
      const row = store.db.prepare('SELECT * FROM pair_requests WHERE id=?').get(requestId);
      if (!row || !safeEqual(digest(data.claimToken), row.claim_hash)) fail(403, 'INVALID_CLAIM', '配对凭据无效。');
      if (row.consumed_at) {
        const existing = store.session(cookieToken(req));
        if (existing && existing.device.id === row.device_id) return json(res, 200, { status: 'approved', device: existing.device, csrfToken: existing.csrfToken });
        fail(409, 'CLAIM_CONSUMED', '此配对已领取，请重新配对。');
      }
      if (row.expires_at <= now()) return json(res, 200, { status: 'expired' });
      if (row.status !== 'approved') return json(res, 200, { status: row.status });
      const device = store.getDevice(row.device_id);
      if (!device) return json(res, 200, { status: 'rejected' });
      const session = store.transaction(() => {
        store.db.prepare('UPDATE pair_requests SET consumed_at=? WHERE id=? AND consumed_at IS NULL').run(now(), row.id);
        return store.createSession(device);
      });
      setSession(res, session);
      broadcast('devices');
      return json(res, 200, { status: 'approved', device, csrfToken: session.csrfToken });
    }

    const session = authenticate(req, false, MUTATING.has(method));
    const ownerOnly = () => { if (session.device.role !== 'owner') fail(403, 'OWNER_REQUIRED', '此操作需要电脑管理者权限。'); };
    if (pathname === '/api/session/logout' && method === 'POST') {
      store.db.prepare('DELETE FROM sessions WHERE token_hash=?').run(session.tokenHash);
      res.setHeader('Set-Cookie', `relay_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0${secure ? '; Secure' : ''}`);
      for (const entry of streams) if (entry.tokenHash === session.tokenHash) entry.res.end();
      return json(res, 200, { ok: true });
    }
    if (pathname === '/api/info' && method === 'GET') {
      const address = server.address();
      return json(res, 200, { version: VERSION, hostName: os.hostname(), urls: connectionUrls(), port: address?.port || 4318,
        secure, maxFileBytes, maxStorageBytes, usedBytes: store.storageBytes(), chunkSize, apkAnalyzerAvailable: Boolean(analyzer),
        ...(session.device.role === 'owner' ? { dataDir: store.dataDir } : {}) });
    }
    if (pathname === '/api/events' && method === 'GET') {
      res.writeHead(200, { 'Content-Type': 'text/event-stream; charset=utf-8', 'Connection': 'keep-alive', 'X-Accel-Buffering': 'no' });
      res.flushHeaders();
      res.write(': connected\n\n');
      const entry = { res, deviceId: session.device.id, tokenHash: session.tokenHash };
      streams.add(entry);
      const heartbeat = setInterval(() => {
        if (closing || res.destroyed || res.writableEnded) return;
        const alive = store.db.prepare(`SELECT 1 FROM sessions s JOIN devices d ON d.id=s.device_id WHERE s.token_hash=? AND s.expires_at>? AND d.revoked_at IS NULL`).get(entry.tokenHash, now());
        if (!alive || res.writableLength > 64 * 1024) res.end();
        else res.write(': heartbeat\n\n');
      }, 15_000);
      heartbeat.unref();
      res.on('close', () => { clearInterval(heartbeat); streams.delete(entry); });
      return;
    }
    if (pathname === '/api/pairing' && method === 'POST') {
      ownerOnly();
      let code;
      do { code = String(randomInt(100000, 1000000)); }
      while (store.db.prepare('SELECT 1 FROM pairings WHERE code_hash=? AND used_at IS NULL AND expires_at>?').get(digest(code), now()));
      const token = secret();
      const expiresAt = new Date(Date.now() + 5 * 60_000).toISOString();
      store.db.prepare('INSERT INTO pairings(id,code_hash,token_hash,created_at,expires_at) VALUES(?,?,?,?,?)')
        .run(id(), digest(code), digest(token), now(), expiresAt);
      return json(res, 201, { code, token, expiresAt, urls: connectionUrls().map(base => `${base}/#pair=${token}`) });
    }
    if (pathname === '/api/pair/requests' && method === 'GET') {
      ownerOnly();
      const rows = store.db.prepare("SELECT id,name,created_at,expires_at,status FROM pair_requests WHERE status='pending' AND expires_at>? ORDER BY created_at DESC").all(now());
      return json(res, 200, { requests: rows.map(row => ({ id: row.id, name: row.name, createdAt: row.created_at, expiresAt: row.expires_at, status: row.status })) });
    }
    let match = /^\/api\/pair\/requests\/([\w-]+)\/(approve|reject)$/.exec(pathname);
    if (match && method === 'POST') {
      ownerOnly();
      store.transaction(() => {
        const row = store.db.prepare('SELECT * FROM pair_requests WHERE id=?').get(match[1]);
        if (!row) fail(404, 'PAIR_REQUEST_NOT_FOUND', '未找到配对请求。');
        if (row.expires_at <= now()) fail(410, 'PAIR_REQUEST_EXPIRED', '配对请求已过期，请重新连接。');
        if (row.status !== 'pending') fail(409, 'PAIR_REQUEST_HANDLED', '配对请求已处理。');
        const device = match[2] === 'approve' ? store.createDevice(row.name) : null;
        store.db.prepare('UPDATE pair_requests SET status=?,device_id=? WHERE id=?').run(device ? 'approved' : 'rejected', device?.id || null, row.id);
      });
      broadcast('pairing'); broadcast('devices');
      return json(res, 200, { ok: true });
    }
    if (pathname === '/api/devices' && method === 'GET') {
      return json(res, 200, { devices: store.db.prepare('SELECT * FROM devices WHERE revoked_at IS NULL ORDER BY created_at').all().map(row => store.device(row)) });
    }
    match = /^\/api\/devices\/([\w-]+)$/.exec(pathname);
    if (match && method === 'DELETE') {
      ownerOnly();
      const device = store.getDevice(match[1]);
      if (!device) fail(404, 'DEVICE_NOT_FOUND', '未找到设备。');
      if (device.role === 'owner') fail(403, 'OWNER_REVOKE_FORBIDDEN', '不能撤销电脑管理者设备。');
      store.transaction(() => {
        store.db.prepare('UPDATE devices SET revoked_at=? WHERE id=?').run(now(), device.id);
        store.db.prepare('DELETE FROM sessions WHERE device_id=?').run(device.id);
      });
      for (const entry of streams) if (entry.deviceId === device.id) entry.res.end();
      broadcast('devices');
      return json(res, 200, { ok: true });
    }
    if (pathname === '/api/items' && method === 'GET') {
      const rawLimit = url.searchParams.get('limit') || '200';
      if (!/^\d+$/.test(rawLimit) || Number(rawLimit) < 1) fail(400, 'INVALID_LIMIT', '列表数量无效。');
      const limit = Math.min(200, Number(rawLimit));
      const before = url.searchParams.get('before');
      const beforeId = url.searchParams.get('beforeId');
      if (before && Number.isNaN(Date.parse(before))) fail(400, 'INVALID_DATE', '时间格式无效。');
      if (beforeId !== null && (!before || !/^[\w-]{1,80}$/.test(beforeId))) fail(400, 'INVALID_CURSOR', '记录游标无效，需要同时提供时间与记录标识。');
      let rows;
      if (beforeId !== null) {
        const timestamp = new Date(before).toISOString();
        rows = store.db.prepare('SELECT * FROM items WHERE created_at<? OR (created_at=? AND id<?) ORDER BY created_at DESC,id DESC LIMIT ?')
          .all(timestamp, timestamp, beforeId, limit);
      } else if (before) {
        // Preserve the legacy strict timestamp-only cursor when no ID is supplied.
        rows = store.db.prepare('SELECT * FROM items WHERE created_at<? ORDER BY created_at DESC,id DESC LIMIT ?').all(new Date(before).toISOString(), limit);
      } else rows = store.db.prepare('SELECT * FROM items ORDER BY created_at DESC,id DESC LIMIT ?').all(limit);
      return json(res, 200, { items: rows.map(row => publicItem(store.item(row))) });
    }
    if (pathname === '/api/items/text' && method === 'POST') {
      const data = await readJson(req);
      const text = contentText(data.text);
      const project = shortText(data.project, '项目', 80).trim();
      const buildId = buildReference(data.buildId);
      const item = store.addItem('text', session.device, project, { text, ...(buildId ? { buildId } : {}) });
      broadcast('items'); return json(res, 201, { item: publicItem(item) });
    }
    if (pathname === '/api/feedback' && method === 'POST') {
      const data = await readJson(req);
      const text = contentText(data.text);
      const project = shortText(data.project, '项目', 80).trim();
      const buildId = buildReference(data.buildId);
      const attachmentIds = data.attachmentIds ?? [];
      if (!Array.isArray(attachmentIds) || attachmentIds.length > 100 || attachmentIds.some(value => typeof value !== 'string' || value.length > 80)) fail(400, 'INVALID_ATTACHMENTS', '附件列表无效，最多关联 100 个文件。');
      for (const attachmentId of attachmentIds) if (store.getItem(attachmentId)?.kind !== 'file') fail(400, 'INVALID_ATTACHMENT', '附件必须是已经传完的文件。');
      const item = store.addItem('feedback', session.device, project, { text, attachmentIds: [...new Set(attachmentIds)], ...(buildId ? { buildId } : {}) });
      broadcast('items'); return json(res, 201, { item: publicItem(item) });
    }
    match = /^\/api\/items\/([\w-]+)$/.exec(pathname);
    if (match && method === 'PATCH') {
      const item = store.getItem(match[1]);
      if (!item) fail(404, 'ITEM_NOT_FOUND', '未找到记录。');
      const data = await readJson(req);
      if (!['project', 'note', 'testStatus'].some(key => Object.hasOwn(data, key))) fail(400, 'EMPTY_UPDATE', '没有可更新的内容。');
      const project = data.project === undefined ? item.project : shortText(data.project, '项目', 80).trim();
      const row = store.db.prepare('SELECT data FROM items WHERE id=?').get(item.id);
      const extra = JSON.parse(row.data);
      if (data.note !== undefined) extra.note = shortText(data.note, '备注', 5000);
      if (data.testStatus !== undefined) {
        if (!item.isApk || !TEST_STATUSES.has(data.testStatus)) fail(400, 'INVALID_TEST_STATUS', '只能为 APK 设置有效的人工确认状态。');
        extra.testStatus = data.testStatus;
      }
      store.db.prepare('UPDATE items SET project=?,data=? WHERE id=?').run(project, JSON.stringify(extra), item.id);
      broadcast('items'); return json(res, 200, { item: publicItem(store.getItem(item.id)) });
    }
    match = /^\/api\/feedback\/([\w-]+)\/export$/.exec(pathname);
    if (match && ['GET', 'HEAD'].includes(method)) {
      const item = store.getItem(match[1]);
      if (!item || item.kind !== 'feedback') fail(404, 'FEEDBACK_NOT_FOUND', '未找到反馈记录。');
      const build = item.buildId ? store.getItem(item.buildId) : null;
      const lines = ['# 真机测试反馈', '', `- 项目：${mdLabel(item.project || '未选择')}`, `- 记录时间：${mdLabel(item.createdAt)}`,
        `- 提交设备名称：${mdLabel(item.deviceName)}（用户自定义名称）`, '- 实际设备型号、系统版本及是否已安装：本记录未自动检测。', '', '## 原始反馈', '', fencedText(item.text), '', '## 关联构建', ''];
      if (build) {
        lines.push(`- 文件：${mdLabel(build.name)}`, `- 构建记录：${mdLabel(build.id)}`, `- SHA-256：${mdLabel(build.sha256)}`,
          `- 包名：${mdLabel(build.apk?.packageName || '未知')}`, `- 版本：${mdLabel(build.apk?.versionName || '未知')}`,
          `- 人工状态：${mdLabel(build.testStatus || 'not-tested')}`, `- 电脑相对文件路径：${mdLabel(`files/${build._storageId}.bin`)}`,
          `- 元数据说明：${mdLabel(build.apk?.note || '未知')}`);
      } else lines.push('未关联 APK；不推断构建、安装或验收结果。');
      lines.push('', '## 附件', '');
      if (!item.attachmentIds?.length) lines.push('无附件。');
      for (const attachmentId of item.attachmentIds || []) {
        const attachment = store.getItem(attachmentId);
        if (attachment) lines.push(`- ${mdLabel(attachment.name)} — 电脑相对路径：${mdLabel(`files/${attachment._storageId}.bin`)}；SHA-256：${mdLabel(attachment.sha256)}`);
      }
      lines.push('', '文件路径相对于随手传的数据目录。内容仅导出到当前设备，没有自动提交到外部平台。', '');
      const body = lines.join('\n');
      res.writeHead(200, { 'Content-Type': 'text/markdown; charset=utf-8', 'Content-Disposition': disposition(`feedback-${item.id}.md`), 'Content-Length': Buffer.byteLength(body) });
      return res.end(method === 'HEAD' ? undefined : body);
    }
    if (pathname === '/api/uploads' && method === 'GET') {
      const rows = store.db.prepare('SELECT * FROM uploads WHERE device_id=? ORDER BY created_at DESC').all(session.device.id);
      return json(res, 200, { uploads: rows.map(row => store.upload(row, chunkSize)) });
    }
    if (pathname === '/api/uploads' && method === 'POST') {
      const data = await readJson(req);
      const name = fileName(data.name);
      const size = data.size;
      if (!Number.isSafeInteger(size) || size < 0) fail(400, 'INVALID_SIZE', '文件大小无效。');
      if (size > maxFileBytes) fail(413, 'FILE_TOO_LARGE', '文件超过单文件大小限制。');
      if (typeof data.sha256 !== 'string' || !/^[\da-f]{64}$/i.test(data.sha256)) fail(400, 'INVALID_SHA256', '需要完整文件的 SHA-256 校验值。');
      const metadata = { name, size, mime: mimeType(data.mime), sha256: data.sha256.toLowerCase(), project: shortText(data.project, '项目', 80).trim() };
      const buildId = buildReference(data.buildId);
      if (buildId) metadata.buildId = buildId;
      const uploadId = id();
      const handle = await open(store.tempPath(uploadId), 'wx', 0o600);
      await handle.close();
      try {
        store.transaction(() => {
          if (store.storageBytes() + size > maxStorageBytes) fail(507, 'STORAGE_QUOTA', '电脑收件箱空间配额不足，请调整配额或取消未完成的传输。');
          store.db.prepare("INSERT INTO uploads(id,device_id,created_at,offset,status,data) VALUES(?,?,?,0,'uploading',?)")
            .run(uploadId, session.device.id, now(), JSON.stringify(metadata));
        });
      } catch (error) { await unlink(store.tempPath(uploadId)).catch(() => {}); throw error; }
      broadcast('uploads'); return json(res, 201, { upload: store.getUpload(uploadId, chunkSize) });
    }
    match = /^\/api\/uploads\/([\w-]+)(\/complete)?$/.exec(pathname);
    if (match) {
      const uploadId = match[1];
      const row = uploadAccess(uploadId, session, method !== 'GET');
      if (!match[2] && method === 'GET') return json(res, 200, { upload: store.upload(row, chunkSize) });
      if (!match[2] && method === 'PATCH') return lockedUpload(uploadId, async () => {
        if (row.status !== 'uploading') fail(409, 'UPLOAD_NOT_ACTIVE', '此传输不可继续写入。');
        const metadata = JSON.parse(row.data);
        const offsetHeader = req.headers['upload-offset'];
        if (typeof offsetHeader !== 'string' || !/^\d+$/.test(offsetHeader) || !Number.isSafeInteger(Number(offsetHeader))) fail(400, 'INVALID_OFFSET', '缺少有效的传输位置。');
        if (Number(offsetHeader) !== row.offset) fail(409, 'OFFSET_MISMATCH', '传输位置已改变，请重新查询进度后继续。', { offset: row.offset });
        if (!/^application\/octet-stream(?:\s*;|$)/i.test(req.headers['content-type'] || '')) fail(415, 'BINARY_REQUIRED', '文件分片必须使用二进制流。');
        const body = await readBody(req, Math.min(chunkSize, metadata.size - row.offset));
        if (!body.length) fail(400, 'EMPTY_CHUNK', '文件分片不能为空。');
        const handle = await open(store.tempPath(uploadId), 'r+');
        try {
          const info = await handle.stat();
          if (info.size < row.offset) fail(409, 'UPLOAD_CORRUPTED', '临时文件已改变，请取消后重新传输。');
          if (info.size > row.offset) await handle.truncate(row.offset);
          let written = 0;
          while (written < body.length) {
            const result = await handle.write(body, written, body.length - written, row.offset + written);
            if (!result.bytesWritten) throw Object.assign(new Error('Unable to write upload'), { code: 'EIO' });
            written += result.bytesWritten;
          }
          await handle.sync();
          store.db.prepare('UPDATE uploads SET offset=? WHERE id=?').run(row.offset + body.length, uploadId);
        } catch (error) {
          await handle.truncate(row.offset).catch(() => {});
          throw error;
        } finally { await handle.close(); }
        broadcast('uploads'); return json(res, 200, { upload: store.getUpload(uploadId, chunkSize) });
      });
      if (match[2] && method === 'POST') return lockedUpload(uploadId, async () => {
        await readJson(req);
        if (row.status === 'complete') return json(res, 200, { item: publicItem(store.getItem(row.item_id)) });
        if (row.status !== 'uploading') fail(409, 'UPLOAD_NOT_ACTIVE', '此传输已失败，请取消后重新上传。');
        const metadata = JSON.parse(row.data);
        if (row.offset !== metadata.size) fail(409, 'UPLOAD_INCOMPLETE', '文件尚未完整传输。', { offset: row.offset });
        const info = await stat(store.tempPath(uploadId));
        if (info.size !== metadata.size) fail(409, 'UPLOAD_CORRUPTED', '临时文件大小不一致，请重新传输。');
        const hash = createHash('sha256');
        for await (const chunk of createReadStream(store.tempPath(uploadId))) hash.update(chunk);
        const sha256 = hash.digest('hex');
        if (!safeEqual(sha256, metadata.sha256)) {
          await unlink(store.tempPath(uploadId));
          store.db.prepare("UPDATE uploads SET status='failed' WHERE id=?").run(uploadId);
          broadcast('uploads');
          fail(422, 'HASH_MISMATCH', '文件校验失败，未发布到收件箱，请重新选择原文件上传。');
        }
        const isApk = /\.apk$/i.test(metadata.name);
        let apk = isApk ? await inspectManifestApk(store.tempPath(uploadId)) : undefined;
        if (isApk && apk.status !== 'available' && analyzer) {
          const sdkResult = await inspectSdkApk(analyzer, store.tempPath(uploadId));
          if (sdkResult.packageName) apk = { ...sdkResult, status: 'available', source: 'android-sdk' };
        }
        const detectedMime = await previewMime(store.tempPath(uploadId), metadata.name, metadata.mime);
        await rename(store.tempPath(uploadId), store.filePath(uploadId));
        let item;
        try {
          item = store.transaction(() => {
            const data = { name: metadata.name, size: metadata.size, mime: metadata.mime, sha256, _storageId: uploadId,
              _previewMime: detectedMime, ...(metadata.buildId ? { buildId: metadata.buildId } : {}), isApk,
              ...(isApk ? { apk, testStatus: 'not-tested' } : {}) };
            const created = store.addItem('file', session.device, metadata.project, data);
            store.db.prepare("UPDATE uploads SET status='complete',item_id=? WHERE id=?").run(created.id, uploadId);
            return created;
          });
        } catch (error) {
          await rename(store.filePath(uploadId), store.tempPath(uploadId)).catch(() => {});
          throw error;
        }
        broadcast('items'); broadcast('uploads'); return json(res, 200, { item: publicItem(item) });
      });
      if (!match[2] && method === 'DELETE') return lockedUpload(uploadId, async () => {
        if (row.status === 'complete') fail(409, 'COMPLETED_FILE', '已完成的文件不能通过取消传输删除。');
        await unlink(store.tempPath(uploadId)).catch(error => { if (error.code !== 'ENOENT') throw error; });
        store.db.prepare('DELETE FROM uploads WHERE id=?').run(uploadId);
        broadcast('uploads'); return json(res, 200, { ok: true });
      });
    }
    match = /^\/api\/files\/([\w-]+)(\/(preview|location))?$/.exec(pathname);
    if (match && ['GET', 'HEAD'].includes(method)) {
      if (match[3] === 'location') {
        ownerOnly();
        const item = store.getItem(match[1]);
        if (!item || item.kind !== 'file') fail(404, 'FILE_NOT_FOUND', '未找到文件。');
        return json(res, 200, { path: store.filePath(item._storageId) });
      }
      return serveFile(req, res, match[1], match[3] === 'preview');
    }
    fail(404, 'NOT_FOUND', '未找到此接口。');
  }

  function errorResponse(error, req, res) {
    if (res.destroyed || res.writableEnded) return;
    if (res.headersSent) { res.destroy(); return; }
    let status = error.status || 500;
    let code = error.code || 'INTERNAL_ERROR';
    let message = error instanceof HttpError ? error.message : '电脑处理请求时遇到问题，请重试或查看本机状态。';
    if (['ENOSPC', 'EDQUOT', 'SQLITE_FULL'].includes(error.code)) { status = 507; code = 'DISK_FULL'; message = '电脑磁盘空间不足，本次内容未保存。'; }
    else if (['EACCES', 'EPERM', 'EROFS'].includes(error.code)) { status = 500; code = 'STORAGE_PERMISSION'; message = '电脑数据目录不可写，请检查本机权限。'; }
    else if (!(error instanceof HttpError)) code = 'INTERNAL_ERROR';
    if (error.details?.retryAfter) res.setHeader('Retry-After', String(error.details.retryAfter));
    req.resume();
    json(res, status, { error: { code, message, ...(error instanceof HttpError && error.details ? { details: error.details } : {}) } });
  }

  const handler = (req, res) => {
    const task = route(req, res).catch(error => errorResponse(error, req, res)).finally(() => requests.delete(task));
    requests.add(task);
  };
  let server;
  try { server = secure ? https.createServer(options.tls, handler) : http.createServer(handler); }
  catch (error) { store.close(); throw error; }
  server.requestTimeout = 120_000;
  server.headersTimeout = 30_000;
  server.keepAliveTimeout = 5000;
  server.maxHeadersCount = 100;
  server.on('clientError', (_error, socket) => { if (socket.writable) socket.end('HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n'); });

  return {
    server, store, ownerSecret: store.ownerSecret,
    listen(port = 0, host = '127.0.0.1') {
      return new Promise((resolve, reject) => {
        if (closing) return reject(new Error('Server is closed'));
        const onError = error => { server.off('listening', onListening); reject(error); };
        const onListening = () => { server.off('error', onError); resolve(server.address()); };
        server.once('error', onError); server.once('listening', onListening); server.listen(port, host);
      });
    },
    close() {
      if (closedPromise) return closedPromise;
      closing = true;
      closedPromise = (async () => {
        for (const entry of streams) entry.res.end();
        const stopped = new Promise((resolve, reject) => {
          if (!server.listening) return resolve();
          server.close(error => error ? reject(error) : resolve());
          server.closeAllConnections();
        });
        await Promise.allSettled([...requests]);
        await stopped;
        store.close();
      })();
      return closedPromise;
    }
  };
}
