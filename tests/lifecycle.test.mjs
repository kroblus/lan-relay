import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir, hostname } from 'node:os';
import { join } from 'node:path';
import { randomUUID, createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { createRelayServer } from '../src/server.mjs';

test('a second instance cannot recover or modify an active instance data directory; close releases ownership', async t => {
  const dataDir = await mkdtemp(join(tmpdir(), 'lan-relay-lifecycle-'));
  let app = await createRelayServer({ dataDir });
  t.after(async () => { await app?.close(); await rm(dataDir, { recursive: true, force: true }); });
  const uploadId = randomUUID();
  const bytes = Buffer.from('savedNEW');
  const owner = app.store.owner('生命周期测试电脑');
  app.store.db.prepare("INSERT INTO uploads(id,device_id,created_at,offset,status,data) VALUES(?,?,?,5,'uploading',?)")
    .run(uploadId, owner.id, new Date().toISOString(), JSON.stringify({ name: 'in-flight.bin', size: bytes.length, mime: 'application/octet-stream',
      sha256: createHash('sha256').update(bytes).digest('hex'), project: '' }));
  // The active process may have written an as-yet uncommitted tail. A duplicate
  // launcher must not enter startup recovery and truncate that live write.
  const temporaryFile = app.store.tempPath(uploadId);
  await writeFile(temporaryFile, bytes);
  await assert.rejects(createRelayServer({ dataDir }), /已有随手传服务运行/);
  assert.deepEqual(await readFile(temporaryFile), bytes);
  assert.equal(app.store.getUpload(uploadId, 4).offset, 5);
  const originalOwnerSecret = app.ownerSecret;
  await app.close();
  app = null;
  await assert.rejects(readFile(join(dataDir, 'process-lock')), { code: 'ENOENT' });
  app = await createRelayServer({ dataDir });
  assert.equal(app.ownerSecret, originalOwnerSecret);
  assert.equal((await readFile(temporaryFile)).toString(), 'saved');
  assert.equal(app.store.getUpload(uploadId, 4).offset, 5);
});

test('a lock left by a confirmed exited process is recovered and the replacement lock is released on close', async t => {
  const dataDir = await mkdtemp(join(tmpdir(), 'lan-relay-stale-lock-'));
  let app;
  t.after(async () => { await app?.close(); await rm(dataDir, { recursive: true, force: true }); });
  const exited = spawnSync(process.execPath, ['-e', ''], { encoding: 'utf8' });
  assert.equal(exited.status, 0, 'synthetic child must exit before creating a stale lock fixture');
  assert.ok(Number.isSafeInteger(exited.pid) && exited.pid > 0);
  assert.throws(() => process.kill(exited.pid, 0), { code: 'ESRCH' });
  const lockPath = join(dataDir, 'process-lock');
  await writeFile(lockPath, JSON.stringify({ pid: exited.pid, host: hostname(), token: 'synthetic-exited-process' }));
  app = await createRelayServer({ dataDir });
  const currentLock = JSON.parse(await readFile(lockPath, 'utf8'));
  assert.equal(currentLock.pid, process.pid);
  assert.notEqual(currentLock.token, 'synthetic-exited-process');
  await app.close();
  app = null;
  await assert.rejects(readFile(lockPath), { code: 'ENOENT' });
});
