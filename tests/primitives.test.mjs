import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash, randomBytes } from 'node:crypto';
import { Worker } from 'node:worker_threads';
import { once } from 'node:events';
import { Sha256 } from '../public/sha256.mjs';
import { qrSvg } from '../public/qr.mjs';

const expectedHash = bytes => createHash('sha256').update(bytes).digest('hex');
const encode = text => new TextEncoder().encode(text);

test('SHA-256 agrees with published empty, abc and million-a vectors', () => {
  const vectors = [
    ['', 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'],
    ['abc', 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad'],
    ['a'.repeat(1000000), 'cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0'],
  ];
  for (const [text, digest] of vectors) assert.equal(new Sha256().update(encode(text)).hex(), digest);
});

test('SHA-256 handles UTF-8, subarray offsets and all padding boundaries', () => {
  const unicode = encode('随手传：康养通 APK / Привет 🌿\n\0字节');
  assert.equal(new Sha256().update(unicode).hex(), expectedHash(unicode));
  for (const length of [0, 1, 55, 56, 57, 63, 64, 65, 119, 120, 127, 128, 129, 1024, 8193]) {
    const allocation = randomBytes(length + 17);
    const bytes = allocation.subarray(9, 9 + length);
    const hash = new Sha256();
    for (let i = 0; i < bytes.length; i += 7) hash.update(bytes.subarray(i, i + 7));
    assert.equal(hash.hex(), expectedHash(bytes), `length ${length}`);
  }
});

test('SHA-256 multi-chunk files match Node crypto without retaining caller buffers', () => {
  const hash = new Sha256();
  const native = createHash('sha256');
  for (const length of [1, 63, 64, 65, 65537, 2 * 1024 * 1024, 13]) {
    const bytes = randomBytes(length);
    native.update(bytes);
    assert.equal(hash.update(bytes), hash);
    bytes.fill(0);
  }
  assert.equal(hash.hex(), native.digest('hex'));
});

test('SHA-256 digest snapshots are repeatable and do not finalize the stream', () => {
  const hash = new Sha256().update(encode('ab'));
  assert.equal(hash.hex(), expectedHash('ab'));
  assert.equal(hash.hex(), expectedHash('ab'));
  hash.update(encode('c'));
  assert.equal(hash.hex(), expectedHash('abc'));
  assert.throws(() => hash.update('text'), TypeError);
});

test('SHA-256 crosses the 512 MiB bit-length boundary with bounded memory', () => {
  const part = new Uint8Array(1024 * 1024).fill(0xa5);
  const hash = new Sha256();
  const native = createHash('sha256');
  for (let i = 0; i < 512; i++) {
    hash.update(part);
    native.update(part);
  }
  hash.update(part.subarray(0, 17));
  native.update(part.subarray(0, 17));
  assert.equal(hash.hex(), native.digest('hex'));
});

function newBrowserCompatibleWorker() {
  // Adapt only browser message plumbing; the shipped worker and Blob reads
  // execute unchanged in a real separate Node worker thread.
  return new Worker(`
    const { parentPort, workerData } = require('node:worker_threads');
    globalThis.self = globalThis;
    globalThis.postMessage = message => parentPort.postMessage(message);
    import(workerData.moduleUrl).then(() => {
      parentPort.on('message', data => globalThis.onmessage({ data }));
      parentPort.postMessage({ ready: true });
    });
  `, { eval: true, workerData: { moduleUrl: new URL('../public/hash-worker.mjs', import.meta.url).href } });
}

test('hash worker reports bounded progress, correct large/empty digests, and recoverable errors', { timeout: 15000 }, async t => {
  const worker = newBrowserCompatibleWorker();
  t.after(() => worker.terminate());
  const [ready] = await once(worker, 'message');
  assert.deepEqual(ready, { ready: true });
  const bytes = randomBytes(2 * 1024 * 1024 + 137);
  const messages = [];
  const complete = new Promise((resolve, reject) => {
    worker.on('error', reject);
    worker.on('message', message => {
      messages.push(message);
      if (message.id === 'empty' && message.sha256) resolve();
    });
  });
  worker.postMessage({ id: 'invalid', file: 'not a file' });
  worker.postMessage({ id: 'large', file: new Blob([bytes]) });
  worker.postMessage({ id: 'empty', file: new Blob([]) });
  await complete;
  assert.equal(messages.find(message => message.id === 'large' && message.sha256)?.sha256, expectedHash(bytes));
  assert.equal(messages.find(message => message.id === 'empty' && message.sha256)?.sha256, expectedHash(''));
  assert.equal(typeof messages.find(message => message.id === 'invalid')?.error, 'string');
  for (const id of ['large', 'empty']) {
    const progress = messages.filter(message => message.id === id && message.progress !== undefined).map(message => message.progress);
    assert.equal(progress[0], 0);
    assert.equal(progress.at(-1), 1);
    assert.ok(progress.every((value, i) => value >= 0 && value <= 1 && (!i || value >= progress[i - 1])));
  }
  assert.ok(messages.filter(message => message.id === 'large' && message.progress !== undefined).length >= 3);
});

const QR_CASES = [
  'http://192.168.1.20:4318/#pair=abcdefghijklmnopqrstuvwxyz0123456789',
  'https://example.local/配对?项目=康养通&设备=安卓🌿',
  'https://preview.example.test/' + 'x'.repeat(200 - 'https://preview.example.test/'.length),
  'x'.repeat(256),
];
// These exact matrices were independently rasterized by sharp and decoded
// back to QR_CASES by jsQR (cozmo/jsQR), not by this generator. No decoder
// is shipped or needed at runtime. A mismatch requires re-decoding a fixture.
const DECODED_MATRIX_DIGESTS = [
  '0958e33cf9758c6af8adace1df1b0b32aead17629e2fd79b6b0affafca267ae3',
  '1bfeb1f39ccc46688b95e6152e118ba39098a20d900e60bb5208c79b563bf721',
  'a1fdb4c9d4a8c3d57514f40f7a616883fd4de3e6f257f18c53e39c25a3b1023c',
  '6fde58500ae219cdb43f82d399ea5f16b49b2d0d65c7d9fb4445b98adb7e8d7d',
];

test('QR SVG covers LAN, UTF-8 and 200-byte preview URLs with a four-module quiet zone', () => {
  for (const [caseIndex, content] of QR_CASES.entries()) {
    const svg = qrSvg(content);
    const viewBox = svg.match(/viewBox="0 0 (\d+) (\d+)"/);
    assert.ok(viewBox);
    const side = Number(viewBox[1]);
    assert.equal(side, Number(viewBox[2]));
    assert.equal((side - 8 - 17) % 4, 0);
    assert.ok(side >= 29 && side <= 73);
    const modules = Array.from(svg.matchAll(/M(\d+),(\d+)h1v1h-1z/g), match => [Number(match[1]), Number(match[2])]);
    assert.ok(modules.length > 100);
    assert.equal(expectedHash(JSON.stringify(modules)), DECODED_MATRIX_DIGESTS[caseIndex]);
    assert.ok(modules.every(([x, y]) => x >= 4 && y >= 4 && x < side - 4 && y < side - 4));
    assert.equal(Math.min(...modules.map(([x]) => x)), 4);
    assert.equal(Math.min(...modules.map(([, y]) => y)), 4);
    assert.match(svg, /fill="#fff"/);
  }
});

test('QR SVG never interpolates user text and rejects excessive UTF-8 bytes', () => {
  const svg = qrSvg('</path><script>alert(1)</script><image href="https://bad.invalid/"/>');
  assert.doesNotMatch(svg, /<script|<image|bad\.invalid|alert\(1\)/);
  assert.throws(() => qrSvg('x'.repeat(257)), RangeError);
  assert.throws(() => qrSvg('汉'.repeat(86)), RangeError);
  assert.throws(() => qrSvg(null), TypeError);
});
