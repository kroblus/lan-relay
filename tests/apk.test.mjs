// All archives below are synthetic metadata fixtures, NOT installable APKs.
import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm, open } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { crc32, deflateRawSync } from 'node:zlib';
import { inspectApk } from '../src/apk.mjs';

const NONE = 0xffffffff;
const NS = 'http://schemas.android.com/apk/res/android';
const attr = (name, type, value, ns = NS) => ({ name, type, value, ns });
const str = (name, value, ns = NS) => attr(name, 3, value, ns);
function length8(n) { return Buffer.from(n < 128 ? [n] : [0x80 | (n >>> 8), n & 255]); }
function length16(n) {
  const b = Buffer.alloc(n < 32768 ? 2 : 4);
  if (n < 32768) b.writeUInt16LE(n);
  else { b.writeUInt16LE(0x8000 | (n >>> 16)); b.writeUInt16LE(n & 65535, 2); }
  return b;
}

function axml({ utf8 = true, versionName = '1.2-测试🌿', versionReference = false, debuggable = true,
  rootExtras = [], appExtras = [], noDebug = false, fakeNamespace = false, minSdk = 26, targetSdk = 36 } = {}) {
  const strings = [], indices = new Map();
  const s = value => {
    if (value === null) return NONE;
    if (!indices.has(value)) { indices.set(value, strings.length); strings.push(value); }
    return indices.get(value);
  };
  function start(name, attrs) {
    const out = Buffer.alloc(36 + attrs.length * 20);
    out.writeUInt16LE(0x0102, 0); out.writeUInt16LE(16, 2); out.writeUInt32LE(out.length, 4);
    out.writeUInt32LE(1, 8); out.writeUInt32LE(NONE, 12);
    out.writeUInt32LE(NONE, 16); out.writeUInt32LE(s(name), 20);
    out.writeUInt16LE(20, 24); out.writeUInt16LE(20, 26); out.writeUInt16LE(attrs.length, 28);
    attrs.forEach((a, i) => {
      const at = 36 + i * 20, value = a.type === 3 ? s(a.value) : a.value;
      out.writeUInt32LE(s(a.ns), at); out.writeUInt32LE(s(a.name), at + 4);
      out.writeUInt32LE(a.type === 3 ? value : NONE, at + 8);
      out.writeUInt16LE(8, at + 12); out[at + 15] = a.type; out.writeUInt32LE(value >>> 0, at + 16);
    });
    return out;
  }
  function end(name) {
    const out = Buffer.alloc(24);
    out.writeUInt16LE(0x0103, 0); out.writeUInt16LE(16, 2); out.writeUInt32LE(24, 4);
    out.writeUInt32LE(1, 8); out.writeUInt32LE(NONE, 12);
    out.writeUInt32LE(NONE, 16); out.writeUInt32LE(s(name), 20);
    return out;
  }
  const nodes = [
    start('manifest', [str('package', 'com.example.synthetic', null),
      versionReference ? attr('versionName', 1, 0x7f010001) : str('versionName', versionName),
      attr('versionCode', 0x11, 10203, fakeNamespace ? 'urn:fake-android' : NS), ...rootExtras]),
    start('uses-sdk', [typeof minSdk === 'number' ? attr('minSdkVersion', 0x10, minSdk) : str('minSdkVersion', minSdk),
      typeof targetSdk === 'number' ? attr('targetSdkVersion', 0x10, targetSdk) : str('targetSdkVersion', targetSdk)]),
    end('uses-sdk'),
    start('application', [...(noDebug ? [] : [attr('debuggable', 0x12, debuggable ? 0xffffffff : 0)]), ...appExtras]),
    end('application'), end('manifest'),
  ];
  const chunks = strings.map(value => {
    const data = Buffer.from(value, utf8 ? 'utf8' : 'utf16le');
    return Buffer.concat(utf8 ? [length8(value.length), length8(data.length), data, Buffer.alloc(1)] : [length16(value.length), data, Buffer.alloc(2)]);
  });
  const dataLength = chunks.reduce((sum, b) => sum + b.length, 0);
  const stringStart = 28 + strings.length * 4;
  const pool = Buffer.alloc(Math.ceil((stringStart + dataLength) / 4) * 4);
  pool.writeUInt16LE(1, 0); pool.writeUInt16LE(28, 2); pool.writeUInt32LE(pool.length, 4);
  pool.writeUInt32LE(strings.length, 8); pool.writeUInt32LE(utf8 ? 0x100 : 0, 16); pool.writeUInt32LE(stringStart, 20);
  let at = 0;
  chunks.forEach((chunk, index) => { pool.writeUInt32LE(at, 28 + index * 4); chunk.copy(pool, stringStart + at); at += chunk.length; });
  const root = Buffer.alloc(8);
  root.writeUInt16LE(3, 0); root.writeUInt16LE(8, 2); root.writeUInt32LE(8 + pool.length + nodes.reduce((sum, b) => sum + b.length, 0), 4);
  return Buffer.concat([root, pool, ...nodes]);
}

function zip(entries) {
  const locals = [], central = [];
  let offset = 0;
  for (const e of entries) {
    const name = Buffer.from(e.name ?? 'AndroidManifest.xml');
    const data = e.data;
    const method = e.method ?? 8, flags = e.flags ?? 0;
    const packed = e.packed ?? (method === 8 ? deflateRawSync(data) : data);
    const size = e.declaredSize ?? data.length, crc = e.crc ?? crc32(data);
    const extra = e.extra ?? Buffer.alloc(0), localExtra = e.localExtra ?? extra;
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50); local.writeUInt16LE(20, 4); local.writeUInt16LE(flags, 6); local.writeUInt16LE(method, 8);
    if (!(flags & 8)) { local.writeUInt32LE(crc, 14); local.writeUInt32LE(packed.length, 18); local.writeUInt32LE(size, 22); }
    local.writeUInt16LE(name.length, 26); local.writeUInt16LE(localExtra.length, 28);
    const descriptor = Buffer.alloc(flags & 8 ? 16 : 0);
    if (descriptor.length) { descriptor.writeUInt32LE(0x08074b50); descriptor.writeUInt32LE(crc, 4); descriptor.writeUInt32LE(packed.length, 8); descriptor.writeUInt32LE(size, 12); }
    locals.push(local, name, localExtra, packed, descriptor);
    const cd = Buffer.alloc(46);
    cd.writeUInt32LE(0x02014b50); cd.writeUInt16LE(20, 4); cd.writeUInt16LE(20, 6); cd.writeUInt16LE(flags, 8); cd.writeUInt16LE(method, 10);
    cd.writeUInt32LE(crc, 16); cd.writeUInt32LE(packed.length, 20); cd.writeUInt32LE(size, 24);
    cd.writeUInt16LE(name.length, 28); cd.writeUInt16LE(extra.length, 30); cd.writeUInt32LE(offset, 42);
    central.push(cd, name, extra);
    offset += local.length + name.length + localExtra.length + packed.length + descriptor.length;
  }
  const cd = Buffer.concat(central), end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50); end.writeUInt16LE(entries.length, 8); end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(cd.length, 12); end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, cd, end]);
}

async function fixture(t, bytes) {
  const dir = await mkdtemp(join(tmpdir(), 'relay-apk-synthetic-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const path = join(dir, 'synthetic-not-installable.apk');
  await writeFile(path, bytes);
  return path;
}

test('synthetic UTF-8/deflated manifest exposes literal metadata without installation claims', async t => {
  const result = await inspectApk(await fixture(t, zip([{ data: axml(), localExtra: Buffer.alloc(3) }])));
  assert.deepEqual({ ...result, note: undefined }, { status: 'available', source: 'manifest', packageName: 'com.example.synthetic', versionName: '1.2-测试🌿', versionCode: 10203, minSdk: 26, targetSdk: 36, debuggable: true, note: undefined });
  assert.match(result.note, /未验证签名、可安装性/);
  assert.equal(result.installed, undefined);
});

test('UTF-16, stored entries, false booleans and descriptor ZIP records are supported', async t => {
  for (const options of [{ utf8: false, debuggable: false, minSdk: '24', targetSdk: '35' }, { utf8: true }]) {
    const result = await inspectApk(await fixture(t, zip([{ data: axml(options), method: 0, flags: options.utf8 ? 8 : 0 }])));
    assert.equal(result.status, 'available');
    assert.equal(result.versionName, '1.2-测试🌿');
    assert.equal(result.debuggable, options.debuggable ?? true);
    if (!options.utf8) { assert.equal(result.minSdk, 24); assert.equal(result.targetSdk, 35); }
  }
});

test('multi-byte UTF-8 length prefixes are decoded accurately', async t => {
  const name = '演示版本'.repeat(70);
  const result = await inspectApk(await fixture(t, zip([{ data: axml({ versionName: name }) }])));
  assert.equal(result.status, 'available'); assert.equal(result.versionName, name);
});

test('resource references, preview SDK codenames, absent booleans and fake namespaces stay unknown', async t => {
  const result = await inspectApk(await fixture(t, zip([{ data: axml({ versionReference: true, noDebug: true, fakeNamespace: true, minSdk: 'PreviewCodename' }) }])));
  assert.equal(result.status, 'available');
  for (const key of ['versionName', 'versionCode', 'debuggable', 'minSdk']) assert.equal(result[key], undefined);
  assert.equal(result.targetSdk, 36); assert.match(result.note, /资源引用/);
});

test('missing manifests, split manifests, ZIP64 and multidisk ZIP have explicit unavailable notes', async t => {
  const zip64 = zip([{ data: axml() }]); zip64.writeUInt16LE(0xffff, zip64.length - 12);
  const multidisk = zip([{ data: axml() }]); multidisk.writeUInt16LE(1, multidisk.length - 18);
  const cases = [
    [zip([{ name: 'base.apk', data: Buffer.from('not an apk') }]), /未找到/],
    [zip([{ data: axml({ rootExtras: [str('split', 'config.en', null)] }) }]), /分包/],
    [zip64, /ZIP64/], [multidisk, /分卷/],
    [zip([{ data: axml(), extra: Buffer.from([1, 0, 0, 0]) }]), /ZIP64/],
    [zip([{ data: axml(), method: 99 }]), /压缩方法/],
    [zip([{ data: axml(), flags: 1 }]), /加密/],
  ];
  for (const [bytes, pattern] of cases) {
    const result = await inspectApk(await fixture(t, bytes));
    assert.equal(result.status, 'unavailable'); assert.match(result.note, pattern);
  }
});

test('truncated ZIP/AXML and corrupt string indices fail without returning partial facts', async t => {
  const badStrings = axml(); badStrings.writeUInt32LE(0xfffffff0, 8 + 28);
  const badNode = axml(); badNode.writeUInt32LE(NONE, badNode.length - 4);
  const good = zip([{ data: axml() }]);
  for (const bytes of [good.subarray(0, -1), zip([{ data: axml().subarray(0, -7) }]), zip([{ data: badStrings }]), zip([{ data: badNode }])]) {
    const result = await inspectApk(await fixture(t, bytes));
    assert.equal(result.status, 'invalid'); assert.equal(result.packageName, undefined);
  }
});

test('duplicate manifests/attributes, inconsistent headers and CRC corruption are rejected', async t => {
  const headerMismatch = zip([{ data: axml() }]); headerMismatch.writeUInt16LE(0, 8);
  const cases = [
    zip([{ data: axml() }, { data: axml() }]),
    zip([{ data: axml({ rootExtras: [str('package', 'com.other.fake', null)] }) }]),
    zip([{ data: axml(), crc: 0 }]), headerMismatch,
  ];
  for (const bytes of cases) assert.equal((await inspectApk(await fixture(t, bytes))).status, 'invalid');
});

test('compressed/uncompressed size limits and dishonest decompression bombs are bounded', async t => {
  const bomb = Buffer.alloc(4 * 1024 * 1024 + 1);
  for (const bytes of [
    zip([{ data: bomb }]),
    zip([{ data: bomb, declaredSize: 64 }]),
    zip([{ data: axml(), packed: Buffer.alloc(2 * 1024 * 1024 + 1) }]),
  ]) {
    const result = await inspectApk(await fixture(t, bytes));
    assert.equal(result.status, 'invalid'); assert.match(result.note, /上限/);
  }
});

test('large sparse archives read only directory/manifest ranges', async t => {
  const bytes = zip([{ data: axml() }]);
  const oldOffset = bytes.readUInt32LE(bytes.length - 6);
  const tail = Buffer.from(bytes.subarray(oldOffset));
  const sparseOffset = 128 * 1024 * 1024;
  tail.writeUInt32LE(sparseOffset, tail.length - 6);
  const path = await fixture(t, bytes.subarray(0, oldOffset));
  const handle = await open(path, 'r+');
  try { await handle.write(tail, 0, tail.length, sparseOffset); } finally { await handle.close(); }
  const result = await inspectApk(path);
  assert.equal(result.status, 'available'); assert.equal(result.packageName, 'com.example.synthetic');
});

test('missing paths return a non-sensitive unavailable result', async () => {
  const result = await inspectApk(join(tmpdir(), 'relay-definitely-missing-file.apk'));
  assert.equal(result.status, 'unavailable'); assert.equal(result.source, 'manifest');
  assert.doesNotMatch(result.note, /ENOENT|\/tmp\/|Error:/);
});
