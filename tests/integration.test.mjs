import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, readFile, appendFile, truncate, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve, sep } from 'node:path';
import { createHash } from 'node:crypto';
import { request as httpRequest } from 'node:http';
import { createRelayServer } from '../src/server.mjs';

const digest = bytes => createHash('sha256').update(bytes).digest('hex');

class Client {
  constructor(base) { this.base = base; this.cookie = ''; this.csrf = ''; }
  async call(path, { method = 'GET', json, body, headers = {}, csrf = true } = {}) {
    const init = { method, headers: { Origin: this.base, ...headers } };
    if (this.cookie) init.headers.Cookie = this.cookie;
    if (csrf && this.csrf && method !== 'GET' && method !== 'HEAD') init.headers['X-CSRF-Token'] = this.csrf;
    if (json !== undefined) { init.body = JSON.stringify(json); init.headers['Content-Type'] = 'application/json'; }
    else if (body !== undefined) init.body = body;
    const response = await fetch(this.base + path, init);
    const cookie = response.headers.get('set-cookie');
    if (cookie) this.cookie = cookie.split(';')[0];
    return response;
  }
  async json(path, options) {
    const response = await this.call(path, options);
    const body = await response.json();
    assert.ok(response.ok, `${options?.method || 'GET'} ${path}: ${response.status} ${JSON.stringify(body)}`);
    if (body.csrfToken) this.csrf = body.csrfToken;
    return body;
  }
}

async function fixture(t, options = {}) {
  const dataDir = await mkdtemp(join(tmpdir(), 'lan-relay-test-'));
  let app = await createRelayServer({ dataDir, ...options });
  let address = await app.listen(0, '127.0.0.1');
  let base = `http://127.0.0.1:${address.port}`;
  const owner = new Client(base);
  await owner.json('/api/session/owner', { method: 'POST', json: { secret: app.ownerSecret, name: '测试电脑' } });
  const state = {
    app, dataDir, base, owner,
    async restart() {
      await app.close();
      app = await createRelayServer({ dataDir, ...options });
      address = await app.listen(0, '127.0.0.1');
      base = `http://127.0.0.1:${address.port}`;
      state.app = app; state.base = base; owner.base = base;
    },
  };
  t.after(async () => { await app.close(); await rm(dataDir, { recursive: true, force: true }); });
  return state;
}

async function pair(owner, base, name = '测试手机') {
  const pairing = await owner.json('/api/pairing', { method: 'POST', json: {} });
  const peer = new Client(base);
  const pending = await peer.json('/api/pair/requests', { method: 'POST', json: { token: pairing.token, name } });
  assert.equal(pending.status, 'pending');
  const waiting = await peer.json('/api/pair/claim', { method: 'POST', json: { id: pending.id, claimToken: pending.claimToken } });
  assert.equal(waiting.status, 'pending');
  await owner.json(`/api/pair/requests/${pending.id}/approve`, { method: 'POST', json: {} });
  const approved = await peer.json('/api/pair/claim', { method: 'POST', json: { id: pending.id, claimToken: pending.claimToken } });
  assert.equal(approved.status, 'approved');
  assert.equal(approved.device.role, 'peer');
  return { peer, pending, pairing, device: approved.device };
}

async function begin(client, bytes, extras = {}) {
  return (await client.json('/api/uploads', { method: 'POST', json: { name: '样例.bin', size: bytes.length, mime: 'application/octet-stream', sha256: digest(bytes), ...extras } })).upload;
}

async function append(client, upload, bytes, offset = upload.offset) {
  return (await client.json(`/api/uploads/${upload.id}`, { method: 'PATCH', body: bytes, headers: { 'Content-Type': 'application/octet-stream', 'Upload-Offset': String(offset) } })).upload;
}

async function uploadFile(client, bytes, extras = {}) {
  let upload = await begin(client, bytes, extras);
  while (upload.offset < bytes.length) {
    upload = await append(client, upload, bytes.subarray(upload.offset, Math.min(upload.offset + upload.chunkSize, bytes.length)));
  }
  return (await client.json(`/api/uploads/${upload.id}/complete`, { method: 'POST', json: {} })).item;
}

test('unauthenticated visitors cannot read workspace or mint owner credentials', async t => {
  const { base } = await fixture(t);
  const stranger = new Client(base);
  assert.deepEqual(await stranger.json('/api/session'), { authenticated: false });
  for (const path of ['/api/items', '/api/devices', '/api/info', '/api/uploads', '/api/events']) {
    const res = await stranger.call(path);
    assert.ok([401, 403].includes(res.status), `${path} must require a session`);
    await res.text();
  }
  const denied = await stranger.call('/api/session/owner', { method: 'POST', json: { secret: 'not-a-real-secret' } });
  assert.ok([401, 403].includes(denied.status));
  assert.equal((await stranger.json('/api/session')).authenticated, false);
  const health = await stranger.json('/api/health');
  assert.equal(health.ok, true);
  assert.equal(JSON.stringify(health).includes('secret'), false);
});

test('owner-approved pairing is single-use, peer cannot approve or create invitations', async t => {
  const { owner, base } = await fixture(t);
  const { peer, pending, pairing, device } = await pair(owner, base);
  assert.equal((await peer.json('/api/session')).device.name, '测试手机');
  assert.ok((await owner.json('/api/devices')).devices.some(d => d.id === device.id));
  const stranger = new Client(base);
  const reused = await stranger.call('/api/pair/requests', { method: 'POST', json: { token: pairing.token, name: '窃取者' } });
  assert.ok(reused.status >= 400);
  const claimAgain = await stranger.call('/api/pair/claim', { method: 'POST', json: { id: pending.id, claimToken: pending.claimToken } });
  assert.ok(claimAgain.status >= 400 || (await claimAgain.json()).status !== 'approved');
  const newInvitation = await peer.call('/api/pairing', { method: 'POST', json: {} });
  assert.equal(newInvitation.status, 403);
});

test('text preserves multiline content and API mutations require CSRF and same-origin', async t => {
  const { owner } = await fixture(t);
  const text = '中文与Русский\n  const a = "<script>";\n\treturn a;';
  const denied = await owner.call('/api/items/text', { method: 'POST', csrf: false, json: { text } });
  assert.equal(denied.status, 403);
  const hostile = await owner.call('/api/items/text', { method: 'POST', json: { text }, headers: { Origin: 'https://attacker.invalid' } });
  assert.equal(hostile.status, 403);
  const { item } = await owner.json('/api/items/text', { method: 'POST', json: { text, project: '传输测试' } });
  assert.equal(item.text, text);
  assert.equal(item.kind, 'text');
  const { items } = await owner.json('/api/items');
  assert.equal(items.length, 1);
  assert.equal(items[0].text, text);
  const blank = await owner.call('/api/items/text', { method: 'POST', json: { text: '  \n ' } });
  assert.ok(blank.status >= 400);
});

test('host validation rejects DNS rebinding requests', async t => {
  const { base } = await fixture(t);
  const status = await new Promise((resolveStatus, reject) => {
    const req = httpRequest(base + '/api/health', { headers: { Host: 'attacker.invalid' } }, res => { res.resume(); res.on('end', () => resolveStatus(res.statusCode)); });
    req.on('error', reject); req.end();
  });
  assert.ok([400, 403, 421].includes(status), `Host attack returned ${status}`);
});

test('files are immutable, checksum-verified, downloadable with HEAD, byte and suffix ranges', async t => {
  const { owner } = await fixture(t, { chunkSize: 4 });
  const bytes = Buffer.from('0123456789abcdefghij');
  const item = await uploadFile(owner, bytes, { name: '截图 "测试".bin' });
  assert.equal(item.sha256, digest(bytes));
  assert.equal(item.size, bytes.length);
  const response = await owner.call(`/api/files/${item.id}`);
  assert.equal(response.status, 200);
  assert.match(response.headers.get('content-disposition'), /^attachment;/);
  assert.equal(response.headers.get('accept-ranges'), 'bytes');
  const etag = response.headers.get('etag');
  assert.deepEqual(Buffer.from(await response.arrayBuffer()), bytes);
  const head = await owner.call(`/api/files/${item.id}`, { method: 'HEAD' });
  assert.equal(head.headers.get('content-length'), String(bytes.length));
  assert.equal(await head.text(), '');
  const range = await owner.call(`/api/files/${item.id}`, { headers: { Range: 'bytes=3-7' } });
  assert.equal(range.status, 206); assert.equal(await range.text(), '34567');
  assert.equal(range.headers.get('content-range'), 'bytes 3-7/20');
  const suffix = await owner.call(`/api/files/${item.id}`, { headers: { Range: 'bytes=-4' } });
  assert.equal(suffix.status, 206); assert.equal(await suffix.text(), 'ghij');
  const invalid = await owner.call(`/api/files/${item.id}`, { headers: { Range: 'bytes=200-300' } });
  assert.equal(invalid.status, 416);
  const ifRange = await owner.call(`/api/files/${item.id}`, { headers: { Range: 'bytes=0-2', 'If-Range': etag } });
  assert.equal(ifRange.status, 206); assert.equal(await ifRange.text(), '012');
  const changed = await owner.call(`/api/files/${item.id}`, { headers: { Range: 'bytes=0-2', 'If-Range': '"stale"' } });
  assert.equal(changed.status, 200); assert.equal(await changed.text(), bytes.toString());
});

test('upload survives service restart and wrong offsets never append duplicate bytes', async t => {
  const state = await fixture(t, { chunkSize: 8 });
  const bytes = Buffer.from('a resumable upload with stable identity');
  let upload = await begin(state.owner, bytes);
  upload = await append(state.owner, upload, bytes.subarray(0, 8));
  assert.equal(upload.offset, 8);
  const wrong = await state.owner.call(`/api/uploads/${upload.id}`, { method: 'PATCH', body: bytes.subarray(0, 8), headers: { 'Content-Type': 'application/octet-stream', 'Upload-Offset': '0' } });
  assert.equal(wrong.status, 409);
  assert.equal((await state.owner.json(`/api/uploads/${upload.id}`)).upload.offset, 8);
  await state.restart();
  upload = (await state.owner.json(`/api/uploads/${upload.id}`)).upload;
  assert.equal(upload.offset, 8);
  while (upload.offset < bytes.length) upload = await append(state.owner, upload, bytes.subarray(upload.offset, upload.offset + 8));
  const completed = await state.owner.json(`/api/uploads/${upload.id}/complete`, { method: 'POST', json: {} });
  const repeated = await state.owner.json(`/api/uploads/${upload.id}/complete`, { method: 'POST', json: {} });
  assert.equal(completed.item.id, repeated.item.id);
  assert.equal((await state.owner.json('/api/items')).items.length, 1);
  const downloaded = await state.owner.call(`/api/files/${completed.item.id}`);
  assert.deepEqual(Buffer.from(await downloaded.arrayBuffer()), bytes);
});

test('incomplete uploads and wrong hashes cannot become successful feed items', async t => {
  const { owner } = await fixture(t, { chunkSize: 16 });
  const original = Buffer.from('good bytes');
  let upload = await begin(owner, original);
  const incomplete = await owner.call(`/api/uploads/${upload.id}/complete`, { method: 'POST', json: {} });
  assert.ok(incomplete.status >= 400);
  upload = await append(owner, upload, Buffer.from('evil bytes'));
  const corrupt = await owner.call(`/api/uploads/${upload.id}/complete`, { method: 'POST', json: {} });
  assert.ok(corrupt.status >= 400);
  assert.equal((await owner.json('/api/items')).items.length, 0);
  const cancellation = await owner.call(`/api/uploads/${upload.id}`, { method: 'DELETE' });
  assert.ok(cancellation.ok);
});

test('empty files are valid and the browser may not preview uploaded HTML or disguised media', async t => {
  const { owner } = await fixture(t);
  const empty = await uploadFile(owner, Buffer.alloc(0), { name: 'empty.txt', mime: 'text/plain' });
  assert.equal(empty.size, 0);
  assert.equal(await (await owner.call(`/api/files/${empty.id}`)).text(), '');
  const html = await uploadFile(owner, Buffer.from('<script>alert(1)</script>'), { name: 'danger.png', mime: 'image/png' });
  const preview = await owner.call(`/api/files/${html.id}/preview`);
  assert.ok([400, 403, 404, 415].includes(preview.status));
  const attachment = await owner.call(`/api/files/${html.id}`);
  assert.match(attachment.headers.get('content-disposition'), /^attachment;/);
  assert.equal(attachment.headers.get('x-content-type-options'), 'nosniff');
});

test('valid PNG can be previewed while original bytes remain unchanged', async t => {
  const { owner } = await fixture(t);
  const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jWZkAAAAASUVORK5CYII=', 'base64');
  const item = await uploadFile(owner, png, { name: '原图.png', mime: 'image/png' });
  const preview = await owner.call(`/api/files/${item.id}/preview`);
  assert.equal(preview.status, 200);
  assert.equal(preview.headers.get('content-type').split(';')[0], 'image/png');
  assert.deepEqual(Buffer.from(await preview.arrayBuffer()), png);
});

test('APK feedback binds to immutable build and exports facts without invented installation state', async t => {
  const { owner, base } = await fixture(t);
  const { peer } = await pair(owner, base);
  const apk = await uploadFile(owner, Buffer.from('synthetic test fixture, not a real APK'), { name: 'app-debug.apk', project: '界面测试' });
  assert.equal(apk.isApk, true);
  assert.notEqual(apk.testStatus, 'user-confirmed-installed');
  assert.ok(!apk.apk?.packageName, 'Synthetic bytes must not produce a fabricated Android package name');
  const attachment = await uploadFile(peer, Buffer.from('test log\nkeyboard overlaps footer'), { name: '日志.txt', mime: 'text/plain', buildId: apk.id });
  const feedbackText = '更多设置太长；00:12 出现跳动。';
  const { item: feedback } = await peer.json('/api/feedback', { method: 'POST', json: { text: feedbackText, buildId: apk.id, attachmentIds: [attachment.id] } });
  assert.equal(feedback.buildId, apk.id);
  assert.deepEqual(feedback.attachmentIds, [attachment.id]);
  const exported = await owner.call(`/api/feedback/${feedback.id}/export`);
  assert.equal(exported.status, 200);
  const md = await exported.text();
  assert.ok(md.includes(feedbackText)); assert.ok(md.includes(apk.sha256)); assert.ok(md.includes('日志.txt'));
  const { item: marked } = await peer.json(`/api/items/${apk.id}`, { method: 'PATCH', json: { testStatus: 'user-confirmed-installed' } });
  assert.equal(marked.testStatus, 'user-confirmed-installed');
  const forged = await peer.call(`/api/items/${apk.id}`, { method: 'PATCH', json: { testStatus: 'adb-installed' } });
  assert.ok(forged.status >= 400);
  const badReference = await peer.call('/api/feedback', { method: 'POST', json: { text: 'bad reference', buildId: 'nonexistent-build' } });
  assert.ok(badReference.status >= 400);
});

test('peer file transfer is bidirectional and revoked peers immediately lose access', async t => {
  const { owner, base } = await fixture(t);
  const { peer, device } = await pair(owner, base);
  const fromPc = await uploadFile(owner, Buffer.from('computer to phone'));
  const fromPhone = await uploadFile(peer, Buffer.from('phone to computer'), { name: 'phone.txt' });
  assert.equal(await (await peer.call(`/api/files/${fromPc.id}`)).text(), 'computer to phone');
  assert.equal(await (await owner.call(`/api/files/${fromPhone.id}`)).text(), 'phone to computer');
  const forbiddenLocation = await peer.call(`/api/files/${fromPc.id}/location`);
  assert.equal(forbiddenLocation.status, 403);
  await owner.json(`/api/devices/${device.id}`, { method: 'DELETE' });
  const blocked = await peer.call(`/api/files/${fromPc.id}`);
  assert.ok([401, 403].includes(blocked.status));
  assert.equal((await peer.json('/api/session')).authenticated, false);
});

test('file size/storage quota and chunk bounds reject unsafe requests before completion', async t => {
  const { owner } = await fixture(t, { maxFileBytes: 32, maxStorageBytes: 40, chunkSize: 8 });
  const tooLarge = await owner.call('/api/uploads', { method: 'POST', json: { name: 'large.bin', size: 33, sha256: digest(''), mime: 'application/octet-stream' } });
  assert.ok([400, 413, 507].includes(tooLarge.status));
  const bytes = Buffer.alloc(24, 1);
  const upload = await begin(owner, bytes);
  const quota = await owner.call('/api/uploads', { method: 'POST', json: { name: 'another.bin', size: 24, sha256: digest(bytes) } });
  assert.ok([400, 413, 507].includes(quota.status));
  const bigChunk = await owner.call(`/api/uploads/${upload.id}`, { method: 'PATCH', body: bytes, headers: { 'Content-Type': 'application/octet-stream', 'Upload-Offset': '0' } });
  assert.ok([400, 413].includes(bigChunk.status));
  assert.equal((await owner.json(`/api/uploads/${upload.id}`)).upload.offset, 0);
  await owner.json(`/api/uploads/${upload.id}`, { method: 'DELETE' });
  const afterCancel = await begin(owner, bytes);
  assert.equal(afterCancel.offset, 0);
});

test('same-name files remain distinct and stored paths stay under the managed data directory', async t => {
  const { owner, dataDir } = await fixture(t);
  const first = await uploadFile(owner, Buffer.from('one'), { name: 'app-debug.apk' });
  const second = await uploadFile(owner, Buffer.from('two'), { name: 'app-debug.apk' });
  assert.notEqual(first.id, second.id);
  const location = await owner.json(`/api/files/${first.id}/location`);
  assert.ok(resolve(location.path).startsWith(resolve(dataDir) + sep));
  assert.equal(await readFile(location.path, 'utf8'), 'one');
  assert.equal(await (await owner.call(`/api/files/${second.id}`)).text(), 'two');
  const traversal = await owner.call('/api/uploads', { method: 'POST', json: { name: '../../escape.txt', size: 0, sha256: digest('') } });
  if (traversal.ok) {
    const { upload } = await traversal.json();
    const { item } = await owner.json(`/api/uploads/${upload.id}/complete`, { method: 'POST', json: {} });
    const safe = await owner.json(`/api/files/${item.id}/location`);
    assert.ok(resolve(safe.path).startsWith(resolve(dataDir) + sep));
  } else assert.ok([400, 422].includes(traversal.status));
});

test('SSE announces changes only after successful mutations', { timeout: 10000 }, async t => {
  const { owner } = await fixture(t);
  const controller = new AbortController();
  const stream = await fetch(owner.base + '/api/events', { headers: { Cookie: owner.cookie, Origin: owner.base }, signal: controller.signal });
  assert.equal(stream.status, 200);
  assert.match(stream.headers.get('content-type'), /^text\/event-stream/);
  const reader = stream.body.getReader();
  t.after(() => { controller.abort(); reader.cancel().catch(() => {}); });
  await owner.json('/api/items/text', { method: 'POST', json: { text: 'live update' } });
  let text = '';
  while (!text.includes('event: change')) {
    const next = await reader.read();
    if (next.done) break;
    text += new TextDecoder().decode(next.value);
  }
  assert.match(text, /event: change/);
  controller.abort();
});

test('restart truncates an uncommitted file tail and preserves only the acknowledged offset', async t => {
  const state = await fixture(t, { chunkSize: 8 });
  const bytes = Buffer.from('correct complete bytes');
  let upload = await begin(state.owner, bytes);
  upload = await append(state.owner, upload, bytes.subarray(0, 8));
  const temporaryPath = state.app.store.tempPath(upload.id);
  await appendFile(temporaryPath, Buffer.from('uncommitted tail'));
  assert.ok((await stat(temporaryPath)).size > 8);
  await state.restart();
  assert.equal((await stat(temporaryPath)).size, 8);
  upload = (await state.owner.json(`/api/uploads/${upload.id}`)).upload;
  assert.equal(upload.offset, 8);
  while (upload.offset < bytes.length) upload = await append(state.owner, upload, bytes.subarray(upload.offset, upload.offset + 8));
  const { item } = await state.owner.json(`/api/uploads/${upload.id}/complete`, { method: 'POST', json: {} });
  assert.equal(await (await state.owner.call(`/api/files/${item.id}`)).text(), bytes.toString());
});

test('missing committed bytes produce a recoverable failure, never false completion or hidden quota usage', async t => {
  const state = await fixture(t, { chunkSize: 8, maxStorageBytes: 32 });
  const bytes = Buffer.alloc(24, 65);
  let upload = await begin(state.owner, bytes);
  upload = await append(state.owner, upload, bytes.subarray(0, 8));
  const temporaryPath = state.app.store.tempPath(upload.id);
  await truncate(temporaryPath, 3);
  await state.restart();
  const failed = (await state.owner.json(`/api/uploads/${upload.id}`)).upload;
  assert.equal(failed.status, 'failed');
  assert.equal((await state.owner.json('/api/items')).items.length, 0);
  await assert.rejects(stat(temporaryPath), { code: 'ENOENT' });
  const next = await begin(state.owner, bytes);
  assert.equal(next.offset, 0);
});

test('an interrupted binary request cannot commit a partial chunk', { timeout: 10000 }, async t => {
  const { owner } = await fixture(t, { chunkSize: 8 });
  const bytes = Buffer.from('abcdefgh12345678');
  const upload = await begin(owner, bytes);
  await new Promise(resolveRequest => {
    const req = httpRequest(owner.base + `/api/uploads/${upload.id}`, {
      method: 'PATCH', headers: { Cookie: owner.cookie, Origin: owner.base, 'X-CSRF-Token': owner.csrf,
        'Content-Type': 'application/octet-stream', 'Content-Length': '8', 'Upload-Offset': '0' }
    });
    req.on('error', () => {});
    req.on('close', resolveRequest);
    req.flushHeaders(); req.write(Buffer.from('abc'));
    setTimeout(() => req.destroy(), 20);
  });
  assert.equal((await owner.json(`/api/uploads/${upload.id}`)).upload.offset, 0);
  const continued = await append(owner, upload, bytes.subarray(0, 8));
  assert.equal(continued.offset, 8);
});

test('unauthenticated form posts cannot trigger owner login or pairing claims', async t => {
  const { base, app } = await fixture(t);
  for (const path of ['/api/session/owner', '/api/pair/claim']) {
    const response = await fetch(base + path, { method: 'POST', headers: { 'Content-Type': 'text/plain' }, body: JSON.stringify({ secret: app.ownerSecret }) });
    assert.equal(response.status, 415);
  }
});

test('stable time-and-ID feed cursor returns every same-millisecond item exactly once across pages', async t => {
  const { owner, app } = await fixture(t);
  const itemIds = [];
  const timestamp = '2026-11-01T10:00:00.123Z';
  for (let index = 0; index < 7; index++) {
    const { item } = await owner.json('/api/items/text', { method: 'POST', json: { text: `同毫秒分页记录 ${index}` } });
    itemIds.push(item.id);
    app.store.db.prepare('UPDATE items SET created_at=? WHERE id=?').run(timestamp, item.id);
  }
  const seen = [];
  let cursor;
  for (let page = 0; page < 5; page++) {
    const query = new URLSearchParams({ limit: '3', ...(cursor ? { before: cursor.createdAt, beforeId: cursor.id } : {}) });
    const { items } = await owner.json(`/api/items?${query}`);
    if (items.length === 0) break;
    seen.push(...items.map(item => item.id));
    cursor = items.at(-1);
  }
  assert.equal(seen.length, 7);
  assert.equal(new Set(seen).size, 7);
  assert.deepEqual(seen, [...itemIds].sort().reverse());
  const legacy = await owner.json(`/api/items?before=${encodeURIComponent(timestamp)}`);
  assert.deepEqual(legacy.items, [], 'timestamp-only clients keep their original strict-before behavior');
});
