import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, rm, access } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import readline from 'node:readline';

const hostPath = fileURLToPath(new URL('../host.mjs', import.meta.url));

function start(dataDir, extraEnv = {}) {
  const child = spawn(process.execPath, [hostPath], { env: { ...process.env, LAN_RELAY_DATA_DIR: dataDir, LAN_RELAY_PORT: '0', NODE_NO_WARNINGS: '1', ...extraEnv }, stdio: ['pipe', 'pipe', 'pipe'] });
  let stderr = '';
  child.stderr.setEncoding('utf8').on('data', text => { stderr += text; });
  const exit = once(child, 'exit');
  const lines = readline.createInterface({ input: child.stdout });
  let timer;
  const ready = new Promise((resolve, reject) => {
    timer = setTimeout(() => reject(new Error('Desktop host startup timeout')), 8000);
    lines.on('line', text => {
      let message;
      try { message = JSON.parse(text); } catch { reject(new Error('Host sent invalid protocol')); return; }
      if (message.event === 'ready' || message.event === 'error') { clearTimeout(timer); resolve(message); }
    });
    child.on('error', () => reject(new Error('Host process could not start')));
    child.on('exit', () => { clearTimeout(timer); });
  });
  return { child, ready, exit, stderr: () => stderr, async close(eof = false) {
    if (child.exitCode === null && child.signalCode === null) {
      if (!eof) child.stdin.write('shutdown\n');
      child.stdin.end();
    }
    const timer = setTimeout(() => child.kill('SIGKILL'), 8000);
    try { return await exit; } finally { clearTimeout(timer); lines.close(); }
  } };
}

test('desktop host boots the real service, preserves its identity across restart, and exits gracefully', async t => {
  const dataDir = await mkdtemp(join(tmpdir(), 'lan-relay-桌面 数据-'));
  const hosts = [];
  t.after(async () => { for (const host of hosts) await host.close(); await rm(dataDir, { recursive: true, force: true }); });
  let previousSecret;
  for (let pass = 0; pass < 2; pass++) {
    const host = start(dataDir); hosts.push(host);
    const ready = await host.ready;
    assert.equal(ready.event, 'ready');
    const url = new URL(ready.url);
    assert.equal(url.hostname, '127.0.0.1');
    assert.equal((await (await fetch(url.origin + '/api/health')).json()).ok, true);
    assert.equal((await fetch(url.origin + '/api/items')).status, 401);
    const secret = new URLSearchParams(url.hash.slice(1)).get('owner');
    assert.equal(typeof secret, 'string');
    assert.ok(secret.length > 30);
    if (previousSecret) assert.ok(previousSecret === secret, 'Stored owner identity must be preserved');
    previousSecret = secret;
    assert.deepEqual(await host.close(), [0, null]);
    assert.ok(!host.stderr().includes('#owner='), 'Diagnostic channel must not contain the bootstrap link');
    await assert.rejects(access(join(dataDir, 'process-lock')), { code: 'ENOENT' });
  }
});

test('desktop host shuts down when its parent input pipe closes', async t => {
  const dataDir = await mkdtemp(join(tmpdir(), 'lan-relay-desktop-eof-'));
  const host = start(dataDir);
  t.after(async () => { await host.close(); await rm(dataDir, { recursive: true, force: true }); });
  assert.equal((await host.ready).event, 'ready');
  assert.deepEqual(await host.close(true), [0, null]);
  await assert.rejects(access(join(dataDir, 'process-lock')), { code: 'ENOENT' });
});

test('desktop host rejects invalid port without returning owner credentials or leaving a lock', async t => {
  const dataDir = await mkdtemp(join(tmpdir(), 'lan-relay-desktop-bad-'));
  const host = start(dataDir, { LAN_RELAY_PORT: 'not-a-port' });
  t.after(async () => { await host.close(); await rm(dataDir, { recursive: true, force: true }); });
  const result = await host.ready;
  assert.equal(result.event, 'error');
  assert.equal(result.code, 'INVALID_PORT');
  assert.equal(Object.hasOwn(result, 'url'), false);
  assert.deepEqual(await host.exit, [1, null]);
  await assert.rejects(access(join(dataDir, 'process-lock')), { code: 'ENOENT' });
});
