// Original bounded metadata reader; no APK code or external process is run.
// Binary layout references (no third-party parser code is copied):
// https://pkware.cachefly.net/webdocs/casestudies/APPNOTE.TXT
// https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/libs/androidfw/include/androidfw/ResourceTypes.h
import { open } from 'node:fs/promises';
import { inflateRaw, crc32 } from 'node:zlib';
import { promisify } from 'node:util';

const inflate = promisify(inflateRaw);
const MAX_COMPRESSED = 2 * 1024 * 1024;
const MAX_MANIFEST = 4 * 1024 * 1024;
const MAX_DIRECTORY = 16 * 1024 * 1024;
const NO_STRING = 0xffffffff;
const ANDROID_NS = 'http://schemas.android.com/apk/res/android';
const MANIFEST_NAME = Buffer.from('AndroidManifest.xml');
const CAUTION = '仅读取清单元数据；未验证签名、可安装性、实际安装状态或真机兼容性。';

class ApkError extends Error {
  constructor(status, message) { super(message); this.status = status; }
}
const invalid = message => { throw new ApkError('invalid', message); };
const unsupported = message => { throw new ApkError('unavailable', message); };

function bounds(offset, length, total, message = '清单结构越界或被截断。') {
  if (!Number.isSafeInteger(offset) || !Number.isSafeInteger(length) || offset < 0 || length < 0 || offset > total - length) invalid(message);
}

async function readAt(file, offset, length, size) {
  bounds(offset, length, size, 'ZIP 记录越界或文件被截断。');
  const buffer = Buffer.alloc(length);
  let done = 0;
  while (done < length) {
    const { bytesRead } = await file.read(buffer, done, length - done, offset + done);
    if (bytesRead === 0) invalid('ZIP 记录被截断。');
    done += bytesRead;
  }
  return buffer;
}

function checkExtra(bytes) {
  let at = 0;
  while (at < bytes.length) {
    // Older zipalign output can end a local extra field with 1–3 zero bytes.
    if (bytes.length - at < 4 && bytes.subarray(at).every(byte => byte === 0)) break;
    bounds(at, 4, bytes.length, 'ZIP 扩展字段被截断。');
    const type = bytes.readUInt16LE(at), size = bytes.readUInt16LE(at + 2);
    at += 4;
    bounds(at, size, bytes.length, 'ZIP 扩展字段越界。');
    if (type === 0x0001) unsupported('暂不支持 ZIP64 APK。');
    at += size;
  }
}

async function readManifest(file, size) {
  if (size < 22) invalid('文件不是完整的 APK/ZIP。');
  const tailSize = Math.min(size, 22 + 65535 + 20);
  const tail = await readAt(file, size - tailSize, tailSize, size);
  let eocd = -1;
  for (let at = tail.length - 22; at >= 0; at--) {
    if (tail.readUInt32LE(at) === 0x06054b50 && at + 22 + tail.readUInt16LE(at + 20) === tail.length) { eocd = at; break; }
  }
  if (eocd < 0) invalid('找不到完整的 ZIP 中央目录结尾。');
  const endOffset = size - tailSize + eocd;
  const disk = tail.readUInt16LE(eocd + 4), centralDisk = tail.readUInt16LE(eocd + 6);
  const diskEntries = tail.readUInt16LE(eocd + 8), entries = tail.readUInt16LE(eocd + 10);
  const directorySize = tail.readUInt32LE(eocd + 12), directoryOffset = tail.readUInt32LE(eocd + 16);
  if (entries === 0xffff || diskEntries === 0xffff || directorySize === 0xffffffff || directoryOffset === 0xffffffff ||
      (eocd >= 20 && tail.readUInt32LE(eocd - 20) === 0x07064b50)) unsupported('暂不支持 ZIP64 APK。');
  if (disk || centralDisk || diskEntries !== entries) unsupported('暂不支持跨磁盘或分卷 ZIP。');
  if (directorySize > MAX_DIRECTORY) unsupported('ZIP 中央目录超过 16 MiB，已停止元数据解析。');
  bounds(directoryOffset, directorySize, endOffset, 'ZIP 中央目录位置无效。');
  if (entries * 46 > directorySize) invalid('ZIP 中央目录数量与长度不一致。');
  const directory = await readAt(file, directoryOffset, directorySize, size);
  let offset = 0, manifest;
  for (let index = 0; index < entries; index++) {
    bounds(offset, 46, directory.length, 'ZIP 中央目录被截断。');
    if (directory.readUInt32LE(offset) !== 0x02014b50) invalid('ZIP 中央目录签名无效。');
    const flags = directory.readUInt16LE(offset + 8), method = directory.readUInt16LE(offset + 10);
    const crc = directory.readUInt32LE(offset + 16), compressed = directory.readUInt32LE(offset + 20), unpacked = directory.readUInt32LE(offset + 24);
    const nameLength = directory.readUInt16LE(offset + 28), extraLength = directory.readUInt16LE(offset + 30), commentLength = directory.readUInt16LE(offset + 32);
    const entryDisk = directory.readUInt16LE(offset + 34), localOffset = directory.readUInt32LE(offset + 42);
    if ([compressed, unpacked, localOffset].includes(0xffffffff) || entryDisk === 0xffff) unsupported('暂不支持 ZIP64 APK。');
    if (entryDisk) unsupported('暂不支持跨磁盘或分卷 ZIP。');
    const next = offset + 46 + nameLength + extraLength + commentLength;
    bounds(offset, next - offset, directory.length, 'ZIP 中央目录字段越界。');
    const name = directory.subarray(offset + 46, offset + 46 + nameLength);
    checkExtra(directory.subarray(offset + 46 + nameLength, offset + 46 + nameLength + extraLength));
    if (name.equals(MANIFEST_NAME)) {
      if (manifest) invalid('存在重复 AndroidManifest.xml，无法确定清单。');
      manifest = { flags, method, crc, compressed, unpacked, localOffset };
    }
    offset = next;
  }
  if (offset !== directory.length) invalid('ZIP 中央目录有未识别的额外记录。');
  if (!manifest) unsupported('未找到根目录 AndroidManifest.xml；可能不是独立 APK，暂不支持 APK 集合/分包容器。');
  const { flags, method, crc, compressed, unpacked, localOffset } = manifest;
  if (flags & (0x0001 | 0x0040 | 0x2000)) unsupported('暂不支持加密的 APK 清单。');
  if (method !== 0 && method !== 8) unsupported('清单压缩方法不受支持（仅支持 stored/deflate）。');
  if (compressed > MAX_COMPRESSED || unpacked > MAX_MANIFEST) invalid('清单超过安全体积上限（压缩 2 MiB / 解压 4 MiB）。');
  if (unpacked === 0 || (method === 0 && compressed !== unpacked)) invalid('清单长度无效。');
  bounds(localOffset, 30, directoryOffset, '清单本地 ZIP 记录位置无效。');
  const header = await readAt(file, localOffset, 30, size);
  if (header.readUInt32LE(0) !== 0x04034b50 || header.readUInt16LE(6) !== flags || header.readUInt16LE(8) !== method) invalid('ZIP 本地记录与中央目录不一致。');
  if (!(flags & 8) && (header.readUInt32LE(14) !== crc || header.readUInt32LE(18) !== compressed || header.readUInt32LE(22) !== unpacked)) invalid('ZIP 本地记录长度或校验值不一致。');
  const nameLength = header.readUInt16LE(26), extraLength = header.readUInt16LE(28);
  const dataOffset = localOffset + 30 + nameLength + extraLength;
  bounds(localOffset, 30 + nameLength + extraLength + compressed, directoryOffset, '清单数据越界或与中央目录重叠。');
  const localFields = await readAt(file, localOffset + 30, nameLength + extraLength, size);
  if (!localFields.subarray(0, nameLength).equals(MANIFEST_NAME)) invalid('ZIP 本地清单名称不一致。');
  checkExtra(localFields.subarray(nameLength));
  const compressedData = await readAt(file, dataOffset, compressed, size);
  let xml;
  try { xml = method === 0 ? compressedData : await inflate(compressedData, { maxOutputLength: MAX_MANIFEST }); }
  catch { invalid('清单解压失败或解压数据超过 4 MiB 安全上限。'); }
  if (xml.length !== unpacked) invalid('清单实际长度与 ZIP 声明不一致。');
  if (crc32(xml) !== crc) invalid('清单 CRC 校验失败。');
  return xml;
}

function stringPool(buffer, start, headerSize, size) {
  if (headerSize < 28) invalid('AXML 字符串池头部无效。');
  const count = buffer.readUInt32LE(start + 8), styleCount = buffer.readUInt32LE(start + 12);
  const utf8 = Boolean(buffer.readUInt32LE(start + 16) & 0x100);
  const stringsStart = buffer.readUInt32LE(start + 20), stylesStart = buffer.readUInt32LE(start + 24);
  if (count > 65536 || styleCount > 65536) invalid('AXML 字符串数量超过安全上限。');
  const tableEnd = headerSize + 4 * (count + styleCount);
  if (tableEnd > stringsStart || stringsStart > size || (stylesStart && (stylesStart < stringsStart || stylesStart > size))) invalid('AXML 字符串池偏移无效。');
  const end = start + (stylesStart || size);
  const dataStart = start + stringsStart;
  const offsets = new Uint32Array(count);
  for (let index = 0; index < count; index++) {
    offsets[index] = buffer.readUInt32LE(start + headerSize + index * 4);
    if (offsets[index] >= end - dataStart) invalid('AXML 字符串偏移越界。');
  }
  const cache = new Map();
  const decoder = new TextDecoder(utf8 ? 'utf-8' : 'utf-16le', { fatal: true, ignoreBOM: true });
  let decodedBytes = 0;
  return index => {
    if (index === NO_STRING) return null;
    if (index >= count) invalid('AXML 字符串索引越界。');
    if (cache.has(index)) return cache.get(index);
    let at = dataStart + offsets[index];
    function length8() {
      bounds(at, 1, end);
      const first = buffer[at++];
      if (!(first & 0x80)) return first;
      bounds(at, 1, end);
      return (first & 0x7f) * 256 + buffer[at++];
    }
    function length16() {
      bounds(at, 2, end);
      const first = buffer.readUInt16LE(at); at += 2;
      if (!(first & 0x8000)) return first;
      bounds(at, 2, end);
      const second = buffer.readUInt16LE(at); at += 2;
      return (first & 0x7fff) * 65536 + second;
    }
    const characters = utf8 ? length8() : length16();
    const bytes = utf8 ? length8() : characters * 2;
    if (bytes > 65536 || decodedBytes + bytes > 8 * 1024 * 1024) invalid('AXML 字符串超过解析预算。');
    bounds(at, bytes + (utf8 ? 1 : 2), end);
    if (buffer[at + bytes] !== 0 || (!utf8 && buffer[at + bytes + 1] !== 0)) invalid('AXML 字符串缺少终止符。');
    let value;
    try { value = decoder.decode(buffer.subarray(at, at + bytes)); }
    catch { invalid('AXML 字符串编码无效。'); }
    if (value.length !== characters) invalid('AXML 字符串字符数不一致。');
    decodedBytes += bytes;
    cache.set(index, value);
    return value;
  };
}

function parseManifest(xml) {
  bounds(0, 8, xml.length);
  if (xml.readUInt16LE(0) !== 0x0003) unsupported('清单不是受支持的 Android binary AXML 格式。');
  const rootHeader = xml.readUInt16LE(2), rootSize = xml.readUInt32LE(4);
  if (rootHeader < 8 || rootHeader > rootSize || rootSize !== xml.length) invalid('AXML 根区块长度无效。');
  let offset = rootHeader, strings, seenRoot = false, chunks = 0, attributes = 0;
  const stack = [], result = {}, missing = new Set();
  let seenSdk = false, seenApplication = false;

  function decodeValue(at) {
    const type = xml[at + 15], data = xml.readUInt32LE(at + 16);
    const raw = xml.readUInt32LE(at + 8);
    if (type === 0x03) return strings(data);
    if (type === 0x10 || type === 0x11) return data;
    if (type === 0x12) return data !== 0;
    if (type === 0x00 && raw !== NO_STRING) return strings(raw);
    return undefined; // Resource references are intentionally not resolved.
  }

  function setField(field, value) {
    if (value === undefined || value === null) { missing.add(field); return; }
    if (field === 'packageName' || field === 'versionName') {
      if (typeof value !== 'string' || value.length > (field === 'packageName' ? 1024 : 4096) || value.includes('\0')) { missing.add(field); return; }
      result[field] = value;
    } else if (field === 'debuggable') {
      if (typeof value === 'boolean') result[field] = value;
      else if (value === 'true' || value === 'false') result[field] = value === 'true';
      else missing.add(field);
    } else {
      if (typeof value === 'string' && /^\d{1,10}$/.test(value)) value = Number(value);
      if (typeof value === 'number' && Number.isInteger(value) && value >= 0 && value <= 0xffffffff) result[field] = value;
      else missing.add(field);
    }
  }

  while (offset < rootSize) {
    if (++chunks > 100000) invalid('AXML 区块数量超过安全上限。');
    bounds(offset, 8, rootSize);
    const type = xml.readUInt16LE(offset), headerSize = xml.readUInt16LE(offset + 2), size = xml.readUInt32LE(offset + 4);
    if (headerSize < 8 || size < headerSize) invalid('AXML 子区块长度无效。');
    bounds(offset, size, rootSize);
    const end = offset + size;
    if (type === 0x0001) {
      if (strings || seenRoot) invalid('AXML 字符串池重复或位置无效。');
      strings = stringPool(xml, offset, headerSize, size);
    } else if (type === 0x0180) {
      if ((size - headerSize) % 4) invalid('AXML 资源映射长度无效。');
    } else if (type >= 0x0100 && type <= 0x0104) {
      if (!strings || headerSize < 16) invalid('AXML 节点缺少有效字符串池或头部。');
      const ext = offset + headerSize;
      if (type === 0x0102) {
        bounds(ext, 20, end);
        const ns = strings(xml.readUInt32LE(ext)), name = strings(xml.readUInt32LE(ext + 4));
        if (!name) invalid('AXML 元素名称为空。');
        const attributeStart = xml.readUInt16LE(ext + 8), attributeSize = xml.readUInt16LE(ext + 10), count = xml.readUInt16LE(ext + 12);
        if (attributeStart < 20 || attributeSize < 20 || count > 2048 || (attributes += count) > 50000) invalid('AXML 属性布局或数量无效。');
        bounds(ext + attributeStart, attributeSize * count, end);
        if (stack.length === 0) {
          if (seenRoot || name !== 'manifest' || ns !== null) invalid('AXML 根元素不是唯一 manifest。');
          seenRoot = true;
        }
        const manifest = stack.length === 0;
        const sdk = stack.length === 1 && name === 'uses-sdk' && ns === null;
        const application = stack.length === 1 && name === 'application' && ns === null;
        if (sdk && seenSdk) invalid('清单含多个 uses-sdk，无法确定 SDK 信息。');
        if (application && seenApplication) invalid('清单含多个 application。');
        if (sdk) seenSdk = true;
        if (application) seenApplication = true;
        if (stack.length === 1 && name === 'uses-split') unsupported('清单包含分包依赖，暂不支持此类 APK。');
        const relevant = new Set();
        for (let index = 0; index < count; index++) {
          const at = ext + attributeStart + attributeSize * index;
          const valueSize = xml.readUInt16LE(at + 12);
          if (valueSize < 8 || valueSize > attributeSize - 12 || xml[at + 14] !== 0) invalid('AXML 属性值结构无效。');
          const attrNs = strings(xml.readUInt32LE(at)), attrName = strings(xml.readUInt32LE(at + 4));
          let field;
          if (manifest && attrNs === null && attrName === 'package') field = 'packageName';
          if (manifest && attrNs === ANDROID_NS && (attrName === 'versionName' || attrName === 'versionCode')) field = attrName;
          if (sdk && attrNs === ANDROID_NS && (attrName === 'minSdkVersion' || attrName === 'targetSdkVersion')) field = attrName === 'minSdkVersion' ? 'minSdk' : 'targetSdk';
          if (application && attrNs === ANDROID_NS && attrName === 'debuggable') field = 'debuggable';
          if (manifest && ((attrNs === null && (attrName === 'split' || attrName === 'configForSplit')) ||
              (attrNs === ANDROID_NS && (attrName === 'isFeatureSplit' || attrName === 'isSplitRequired')))) {
            const value = decodeValue(at);
            if (value === undefined || (value !== false && value !== '' && value !== 0 && value !== 'false')) unsupported('检测到分包或分包依赖，暂不支持此类 APK。');
          }
          if (field) {
            if (relevant.has(field)) invalid('清单含重复关键属性，无法确定元数据。');
            relevant.add(field);
            setField(field, decodeValue(at));
          }
        }
        stack.push({ name, ns });
        if (stack.length > 128) invalid('AXML 嵌套深度超过安全上限。');
      } else if (type === 0x0103) {
        bounds(ext, 8, end);
        const node = stack.pop();
        if (!node || node.name !== strings(xml.readUInt32LE(ext + 4)) || node.ns !== strings(xml.readUInt32LE(ext))) invalid('AXML 元素结束标记不匹配。');
      } else if (type === 0x0100 || type === 0x0101) {
        bounds(ext, 8, end);
        strings(xml.readUInt32LE(ext)); strings(xml.readUInt32LE(ext + 4));
      } else {
        bounds(ext, 12, end);
      }
    } else unsupported('AXML 含暂不支持的区块类型。');
    offset = end;
  }
  if (!seenRoot || stack.length) invalid('AXML 缺少完整 manifest 根元素。');
  const extra = missing.size ? '部分属性使用资源引用、预览代号或不支持的类型，已保留为未知。' : '';
  return { status: 'available', source: 'manifest', ...result, note: extra + CAUTION };
}

export async function inspectApk(filePath) {
  let file;
  try {
    file = await open(filePath, 'r');
    const stat = await file.stat();
    if (!stat.isFile() || !Number.isSafeInteger(stat.size)) unsupported('无法读取普通 APK 文件。');
    return parseManifest(await readManifest(file, stat.size));
  } catch (error) {
    if (error instanceof ApkError) return { status: error.status, source: 'manifest', note: error.message + CAUTION };
    return { status: 'unavailable', source: 'manifest', note: 'APK 清单暂时无法读取或解析。' + CAUTION };
  } finally {
    await file?.close().catch(() => {});
  }
}
