import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, copyFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { consumeAuthIntent, createAuthIntentQueue, defaultDeviceName, pairingLink, readAuthIntent } from '../public/auth-intent.mjs';

const owner = 'a'.repeat(43);
const token = 'B'.repeat(43);
const tick = () => new Promise((resolve) => setImmediate(resolve));
function deferred() { let resolve; const promise = new Promise((done) => { resolve = done; }); return { promise, resolve }; }

test('authentication fragments distinguish owner, current pairing and legacy invitation links', () => {
  assert.deepEqual(readAuthIntent(`#owner=${owner}`), { kind: 'owner', secret: owner });
  assert.deepEqual(readAuthIntent(`#pair=${token}&code=012345`), { kind: 'pair', token, code: '012345' });
  assert.deepEqual(readAuthIntent(`#pair=${token}`), { kind: 'pair', token, code: '' });
  assert.deepEqual(readAuthIntent('#code=012345'), { kind: 'code', code: '012345' });
  assert.equal(readAuthIntent('#composerText'), null);
  assert.equal(readAuthIntent(''), null);
});

test('ambiguous and malformed authentication fragments fail without including secrets in errors', () => {
  for (const hash of [`#owner=${owner}&pair=${token}`, `#owner=${owner}&owner=${owner}`, `#pair=${token}&code=012345&code=654321`, '#owner=', '#pair=short', `#pair=${token}&code=12`, '#code=六位连接码']) {
    const result = readAuthIntent(hash);
    assert.equal(result.kind, 'invalid');
    assert.equal(result.message.includes(owner), false);
    assert.equal(result.message.includes(token), false);
  }
});

test('consuming a link removes its fragment synchronously while preserving path and query', () => {
  const location = { hash: `#owner=${owner}`, pathname: '/', search: '?view=compact' };
  const calls = [];
  const history = { replaceState: (...args) => calls.push(args) };
  const intent = consumeAuthIntent(location, history);
  assert.equal(intent.kind, 'owner');
  assert.deepEqual(calls, [[null, '', '/?view=compact']]);
  location.hash = '#composerText';
  assert.equal(consumeAuthIntent(location, history), null);
  assert.equal(calls.length, 1);
  location.hash = '#owner=malformed';
  assert.equal(consumeAuthIntent(location, history).kind, 'invalid');
  assert.equal(calls.length, 2, 'even invalid auth material is removed from the address bar');
});

test('new pairing links include only the current token and code in the fragment', () => {
  for (const origin of ['http://192.168.1.20:4318/', 'http://[fd00::12]:4318/', 'https://relay.example/']) {
    const url = new URL(pairingLink(`${origin}#owner=${owner}`, { token, code: '012345' }));
    assert.equal(url.search, '');
    assert.deepEqual(readAuthIntent(url.hash), { kind: 'pair', token, code: '012345' });
    assert.equal(url.hash.includes(owner), false);
  }
  assert.throws(() => pairingLink('javascript:alert(1)', { token, code: '012345' }));
  assert.throws(() => pairingLink('http://user:password@192.168.1.20/', { token, code: '012345' }));
  assert.throws(() => pairingLink('http://192.168.1.20/', { token, code: '123' }));
});

test('device name defaults remain editable and preserve a previously chosen name', () => {
  assert.equal(defaultDeviceName(null), '我的手机');
  assert.equal(defaultDeviceName('  '), '我的手机');
  for (const name of ['我的手机', '我的平板', '另一台电脑', '测试设备 — телефон']) assert.equal(defaultDeviceName(name), name);
  assert.equal(defaultDeviceName('  自定义名称  '), '自定义名称');
  assert.equal(Array.from(defaultDeviceName('📱'.repeat(80))).length, 60);
});

test('a later authentication link waits for the old request but prevents its stale UI result', async () => {
  const gate = deferred();
  const events = [];
  let running = 0;
  const enqueue = createAuthIntentQueue(async (intent, isCurrent) => {
    running += 1;
    assert.equal(running, 1, 'cookie-changing authentication requests must not overlap');
    events.push(`request:${intent}`);
    if (intent === 'old') await gate.promise;
    if (isCurrent()) events.push(`show:${intent}`);
    running -= 1;
  });
  const old = enqueue('old');
  await tick();
  const newest = enqueue('new');
  await tick();
  assert.deepEqual(events, ['request:old']);
  gate.resolve();
  await Promise.all([old, newest]);
  assert.deepEqual(events, ['request:old', 'request:new', 'show:new']);
});

test('multiple new links discard superseded queued intents instead of creating extra sessions', async () => {
  const gate = deferred();
  const events = [];
  const enqueue = createAuthIntentQueue(async (intent, isCurrent) => {
    events.push(`request:${intent}`);
    if (intent === 'first') await gate.promise;
    if (isCurrent()) events.push(`show:${intent}`);
  });
  const first = enqueue('first'); await tick();
  const obsolete = enqueue('obsolete');
  const last = enqueue('last');
  gate.resolve();
  await Promise.all([first, obsolete, last]);
  assert.deepEqual(events, ['request:first', 'request:last', 'show:last']);
});

test('failed authentication does not block a later valid link', async () => {
  const shown = [];
  const enqueue = createAuthIntentQueue(async (intent, isCurrent) => {
    if (intent === 'bad') throw new Error('synthetic rejection');
    if (isCurrent()) shown.push(intent);
  });
  await assert.rejects(enqueue('bad'), /synthetic rejection/);
  await enqueue('good');
  assert.deepEqual(shown, ['good']);
});

test('Windows source launcher preserves the actual Node exit code after its pause', { skip: process.platform !== 'win32' }, async () => {
  const directory = await mkdtemp(path.join(tmpdir(), 'lan relay launcher '));
  try {
    await mkdir(path.join(directory, 'src'));
    const launcher = path.join(directory, 'start.cmd');
    await copyFile(fileURLToPath(new URL('../start.cmd', import.meta.url)), launcher);
    for (const code of [0, 23]) {
      await writeFile(path.join(directory, 'src', 'cli.mjs'), `process.exit(${code});\n`);
      const result = spawnSync(process.env.ComSpec || 'cmd.exe', ['/d', '/s', '/c', `call "${launcher}"`], { cwd: directory, input: '\n', encoding: 'utf8', timeout: 10000, windowsHide: true, windowsVerbatimArguments: true });
      assert.equal(result.error, undefined, result.error?.message);
      assert.equal(result.status, code, result.stdout + result.stderr);
    }
  } finally { await rm(directory, { recursive: true, force: true }); }
});
