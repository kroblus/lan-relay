import { DatabaseSync } from 'node:sqlite';
import { mkdir, readFile, writeFile, chmod, open, unlink, stat, rename } from 'node:fs/promises';
import { readFileSync, unlinkSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { randomBytes, randomUUID, createHash } from 'node:crypto';

export const VERSION = '0.1.0';
export const now = () => new Date().toISOString();
export const id = () => randomUUID();
export const secret = () => randomBytes(32).toString('base64url');
export const digest = value => createHash('sha256').update(value).digest('hex');

async function lockDirectory(dataDir) {
  const lockPath = path.join(dataDir, 'process-lock');
  const token = secret();
  const contents = JSON.stringify({ pid: process.pid, host: os.hostname(), token });
  for (let attempt = 0; attempt < 3; attempt++) {
    try {
      await writeFile(lockPath, contents, { flag: 'wx', mode: 0o600 });
      return { path: lockPath, token };
    } catch (error) {
      if (error.code !== 'EEXIST') throw error;
      let existing;
      try { existing = JSON.parse(await readFile(lockPath, 'utf8')); }
      catch (readError) {
        if (readError.code === 'ENOENT') continue;
        throw new Error('数据目录的进程锁不可读。请确认没有随手传服务运行，再移除数据目录中的 process-lock。');
      }
      if (!Number.isInteger(existing.pid) || existing.pid < 1 || existing.host !== os.hostname()) {
        throw new Error('数据目录已被其他服务占用，或存在无法确认的进程锁。请检查 process-lock。');
      }
      try {
        process.kill(existing.pid, 0);
        throw new Error('此数据目录已有随手传服务运行，请使用已启动的窗口。');
      } catch (pidError) {
        if (pidError.code !== 'ESRCH') throw pidError;
      }
      // Only remove the stale file we observed, never a newer process's lock.
      const current = JSON.parse(await readFile(lockPath, 'utf8'));
      if (current.token === existing.token) await unlink(lockPath).catch(unlinkError => { if (unlinkError.code !== 'ENOENT') throw unlinkError; });
    }
  }
  throw new Error('数据目录正在被另一个随手传进程打开，请稍后重试。');
}

function releaseLock(lock) {
  if (!lock) return;
  try {
    const current = JSON.parse(readFileSync(lock.path, 'utf8'));
    if (current.token === lock.token) unlinkSync(lock.path);
  } catch (error) { if (error.code !== 'ENOENT') throw error; }
}

export class RelayStore {
  constructor(dataDir, db, ownerSecret, lock) {
    this.dataDir = dataDir;
    this.filesDir = path.join(dataDir, 'files');
    this.tempDir = path.join(dataDir, 'temp');
    this.db = db;
    this.ownerSecret = ownerSecret;
    this.lock = lock;
    this.closed = false;
  }

  static async open(directory) {
    const dataDir = path.resolve(directory);
    await mkdir(dataDir, { recursive: true, mode: 0o700 });
    const lock = await lockDirectory(dataDir);
    let openedDatabase;
    try {
    await mkdir(path.join(dataDir, 'files'), { recursive: true, mode: 0o700 });
    await mkdir(path.join(dataDir, 'temp'), { recursive: true, mode: 0o700 });
    const secretFile = path.join(dataDir, 'owner-secret');
    let ownerSecret;
    try {
      ownerSecret = (await readFile(secretFile, 'utf8')).trim();
      if (!/^[A-Za-z0-9_-]{43}$/.test(ownerSecret)) throw new Error('Owner secret file is invalid; restore it rather than silently replacing ownership.');
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
      ownerSecret = secret();
      try { await writeFile(secretFile, `${ownerSecret}\n`, { flag: 'wx', mode: 0o600 }); }
      catch (writeError) {
        if (writeError.code !== 'EEXIST') throw writeError;
        ownerSecret = (await readFile(secretFile, 'utf8')).trim();
        if (!/^[A-Za-z0-9_-]{43}$/.test(ownerSecret)) throw new Error('Owner secret file is invalid.');
      }
    }
    const db = new DatabaseSync(path.join(dataDir, 'relay.sqlite'));
    openedDatabase = db;
    db.exec(`
      PRAGMA journal_mode=WAL;
      PRAGMA foreign_keys=ON;
      PRAGMA busy_timeout=5000;
      CREATE TABLE IF NOT EXISTS devices (
        id TEXT PRIMARY KEY, name TEXT NOT NULL, role TEXT NOT NULL,
        created_at TEXT NOT NULL, last_seen_at TEXT NOT NULL, revoked_at TEXT
      );
      CREATE TABLE IF NOT EXISTS sessions (
        token_hash TEXT PRIMARY KEY, device_id TEXT NOT NULL REFERENCES devices(id),
        csrf TEXT NOT NULL, created_at TEXT NOT NULL, expires_at TEXT NOT NULL
      );
      CREATE TABLE IF NOT EXISTS pairings (
        id TEXT PRIMARY KEY, code_hash TEXT NOT NULL, token_hash TEXT NOT NULL,
        created_at TEXT NOT NULL, expires_at TEXT NOT NULL, used_at TEXT
      );
      CREATE TABLE IF NOT EXISTS pair_requests (
        id TEXT PRIMARY KEY, name TEXT NOT NULL, claim_hash TEXT NOT NULL,
        status TEXT NOT NULL, created_at TEXT NOT NULL, expires_at TEXT NOT NULL,
        device_id TEXT, consumed_at TEXT
      );
      CREATE TABLE IF NOT EXISTS items (
        id TEXT PRIMARY KEY, kind TEXT NOT NULL, created_at TEXT NOT NULL,
        device_id TEXT NOT NULL, device_name TEXT NOT NULL, project TEXT NOT NULL,
        data TEXT NOT NULL
      );
      CREATE INDEX IF NOT EXISTS items_date ON items(created_at DESC);
      CREATE INDEX IF NOT EXISTS items_cursor ON items(created_at DESC, id DESC);
      CREATE TABLE IF NOT EXISTS uploads (
        id TEXT PRIMARY KEY, device_id TEXT NOT NULL, created_at TEXT NOT NULL,
        offset INTEGER NOT NULL, status TEXT NOT NULL, item_id TEXT, data TEXT NOT NULL
      );
      CREATE INDEX IF NOT EXISTS uploads_device ON uploads(device_id, created_at DESC);
    `);
    await chmod(path.join(dataDir, 'relay.sqlite'), 0o600).catch(error => {
      if (process.platform !== 'win32') throw error;
    });
    const store = new RelayStore(dataDir, db, ownerSecret, lock);
    await store.recoverUploads();
    return store;
    } catch (error) {
      try { openedDatabase?.close(); } finally { releaseLock(lock); }
      throw error;
    }
  }

  transaction(fn) {
    this.db.exec('BEGIN IMMEDIATE');
    try { const result = fn(); this.db.exec('COMMIT'); return result; }
    catch (error) { this.db.exec('ROLLBACK'); throw error; }
  }

  device(row) {
    if (!row) return null;
    return { id: row.id, name: row.name, role: row.role, createdAt: row.created_at, lastSeenAt: row.last_seen_at };
  }

  getDevice(deviceId) {
    return this.device(this.db.prepare('SELECT * FROM devices WHERE id=? AND revoked_at IS NULL').get(deviceId));
  }

  createDevice(name, role = 'peer') {
    const device = { id: id(), name, role, createdAt: now(), lastSeenAt: now() };
    this.db.prepare('INSERT INTO devices(id,name,role,created_at,last_seen_at) VALUES(?,?,?,?,?)')
      .run(device.id, name, role, device.createdAt, device.lastSeenAt);
    return device;
  }

  owner(name) {
    const existing = this.db.prepare("SELECT * FROM devices WHERE role='owner' AND revoked_at IS NULL LIMIT 1").get();
    if (existing) {
      if (name) this.db.prepare('UPDATE devices SET name=? WHERE id=?').run(name, existing.id);
      return this.getDevice(existing.id);
    }
    return this.createDevice(name || '我的电脑', 'owner');
  }

  createSession(device) {
    const token = secret();
    const csrfToken = secret();
    const expiresAt = new Date(Date.now() + 90 * 86400_000).toISOString();
    this.db.prepare('INSERT INTO sessions(token_hash,device_id,csrf,created_at,expires_at) VALUES(?,?,?,?,?)')
      .run(digest(token), device.id, csrfToken, now(), expiresAt);
    return { token, csrfToken, device };
  }

  session(token) {
    if (!token || token.length > 256) return null;
    const row = this.db.prepare(`SELECT s.csrf, s.token_hash, d.* FROM sessions s JOIN devices d ON d.id=s.device_id
      WHERE s.token_hash=? AND s.expires_at>? AND d.revoked_at IS NULL`).get(digest(token), now());
    if (!row) return null;
    const lastSeenAt = now();
    this.db.prepare('UPDATE devices SET last_seen_at=? WHERE id=?').run(lastSeenAt, row.id);
    return { device: { ...this.device(row), lastSeenAt }, csrfToken: row.csrf, tokenHash: row.token_hash };
  }

  item(row) {
    if (!row) return null;
    return { id: row.id, kind: row.kind, createdAt: row.created_at, deviceId: row.device_id,
      deviceName: row.device_name, project: row.project, ...JSON.parse(row.data) };
  }

  getItem(itemId) { return this.item(this.db.prepare('SELECT * FROM items WHERE id=?').get(itemId)); }

  addItem(kind, device, project, data, itemId = id()) {
    const createdAt = now();
    this.db.prepare('INSERT INTO items(id,kind,created_at,device_id,device_name,project,data) VALUES(?,?,?,?,?,?,?)')
      .run(itemId, kind, createdAt, device.id, device.name, project, JSON.stringify(data));
    return this.getItem(itemId);
  }

  upload(row, chunkSize) {
    if (!row) return null;
    return { id: row.id, ...JSON.parse(row.data), offset: row.offset, status: row.status,
      createdAt: row.created_at, chunkSize, ...(row.item_id ? { itemId: row.item_id } : {}) };
  }

  getUpload(uploadId, chunkSize) { return this.upload(this.db.prepare('SELECT * FROM uploads WHERE id=?').get(uploadId), chunkSize); }
  tempPath(uploadId) { return path.join(this.tempDir, `${uploadId}.part`); }
  filePath(uploadId) { return path.join(this.filesDir, `${uploadId}.bin`); }

  storageBytes() {
    const rows = this.db.prepare("SELECT data FROM uploads WHERE status IN ('uploading','complete')").all();
    return rows.reduce((total, row) => total + JSON.parse(row.data).size, 0);
  }

  async recoverUploads() {
    const rows = this.db.prepare("SELECT * FROM uploads WHERE status='uploading'").all();
    for (const row of rows) {
      const tempPath = this.tempPath(row.id);
      const finalPath = this.filePath(row.id);
      try {
        let handle;
        try { handle = await open(tempPath, 'r+'); }
        catch (error) {
          if (error.code !== 'ENOENT') throw error;
          // Rename can complete immediately before a crash, prior to the SQLite commit.
          try {
            await stat(finalPath);
            await rename(finalPath, tempPath);
            handle = await open(tempPath, 'r+');
          } catch (finalError) {
            if (finalError.code !== 'ENOENT') throw finalError;
            if (row.offset === 0) handle = await open(tempPath, 'wx', 0o600);
            else {
              this.db.prepare("UPDATE uploads SET status='failed' WHERE id=?").run(row.id);
              continue;
            }
          }
        }
        let corrupt = false;
        try {
          const info = await handle.stat();
          if (info.size < row.offset) corrupt = true;
          else if (info.size !== row.offset) await handle.truncate(row.offset);
        } finally { await handle.close(); }
        if (corrupt) {
          // A short committed file cannot resume truthfully. Remove its generated
          // temporary bytes before releasing reserved quota; keep the failed row.
          await unlink(tempPath);
          this.db.prepare("UPDATE uploads SET status='failed' WHERE id=?").run(row.id);
        }
      } catch (error) {
        throw new Error(`Cannot recover transfer ${row.id}: ${error.code || 'file I/O error'}`, { cause: error });
      }
    }
  }

  close() {
    if (!this.closed) {
      this.closed = true;
      try { this.db.close(); } finally { releaseLock(this.lock); }
    }
  }
}
