import { timingSafeEqual } from 'node:crypto';
import { open, access, stat } from 'node:fs/promises';
import { constants } from 'node:fs';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import path from 'node:path';

const execFileAsync = promisify(execFile);

export class HttpError extends Error {
  constructor(status, code, message, details) {
    super(message);
    this.status = status;
    this.code = code;
    this.details = details;
  }
}

export function fail(status, code, message, details) { throw new HttpError(status, code, message, details); }

export function safeEqual(a, b) {
  if (typeof a !== 'string' || typeof b !== 'string') return false;
  const left = Buffer.from(a), right = Buffer.from(b);
  return left.length === right.length && timingSafeEqual(left, right);
}

export function shortText(value, label, max, fallback = '') {
  if (value === undefined) return fallback;
  if (typeof value !== 'string' || Array.from(value).length > max || value.includes('\0')) {
    fail(400, 'INVALID_INPUT', `${label}格式不正确，最多 ${max} 个字符。`);
  }
  return value;
}

export function deviceName(value) {
  const name = shortText(value, '设备名称', 80).trim();
  if (!name) fail(400, 'INVALID_NAME', '请输入设备名称。');
  return name;
}

export function contentText(value, max = 100000) {
  const text = shortText(value, '文字', max);
  if (!text.trim()) fail(400, 'EMPTY_TEXT', '请输入文字内容。');
  return text;
}

export function fileName(value) {
  if (typeof value !== 'string' || !value.trim() || Array.from(value).length > 512) {
    fail(400, 'INVALID_FILENAME', '文件名不能为空，且不能超过 512 个字符。');
  }
  let name = value.toWellFormed().replace(/[<>:"/\\|?*\x00-\x1f\x7f]/g, '_').trim().replace(/[. ]+$/g, '');
  if (!name || name === '.' || name === '..') fail(400, 'INVALID_FILENAME', '文件名无效。');
  if (/^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\.|$)/i.test(name)) name = `_${name}`;
  name = Array.from(name).slice(0, 240).join('');
  return name;
}

export function mimeType(value) {
  if (value === undefined || value === '') return 'application/octet-stream';
  if (typeof value !== 'string' || value.length > 127 || !/^[a-z\d!#$&^_.+-]+\/[a-z\d!#$&^_.+-]+$/i.test(value)) {
    fail(400, 'INVALID_MIME', '文件类型格式不正确。');
  }
  return value.toLowerCase();
}

export function readBody(req, limit) {
  return new Promise((resolve, reject) => {
    const declared = req.headers['content-length'];
    if (declared && (!/^\d+$/.test(declared) || Number(declared) > limit)) {
      req.resume();
      reject(new HttpError(413, 'BODY_TOO_LARGE', '请求内容超过允许大小。'));
      return;
    }
    const chunks = [];
    let length = 0;
    let settled = false;
    const rejectOnce = error => {
      if (settled) return;
      settled = true;
      chunks.length = 0;
      reject(error);
    };
    req.on('data', chunk => {
      if (settled) return;
      length += chunk.length;
      if (length > limit) {
        rejectOnce(new HttpError(413, 'BODY_TOO_LARGE', '请求内容超过允许大小。'));
        req.resume();
      } else chunks.push(chunk);
    });
    req.on('end', () => {
      if (settled) return;
      settled = true;
      resolve(Buffer.concat(chunks, length));
    });
    req.on('aborted', () => rejectOnce(new HttpError(400, 'INCOMPLETE_REQUEST', '传输中断，请重新连接后继续。')));
    req.on('error', () => rejectOnce(new HttpError(400, 'INCOMPLETE_REQUEST', '请求未完整接收，请重试。')));
  });
}

export async function readJson(req) {
  if (!/^application\/json(?:\s*;|$)/i.test(req.headers['content-type'] || '')) {
    fail(415, 'JSON_REQUIRED', '此操作需要 application/json 请求。');
  }
  const body = await readBody(req, 1024 * 1024);
  try {
    const parsed = JSON.parse(body.toString('utf8'));
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) throw new Error();
    return parsed;
  } catch { fail(400, 'INVALID_JSON', '请求 JSON 格式不正确。'); }
}

export function disposition(name, inline = false) {
  const fallback = name.replace(/[^\x20-\x7e]|["\\]/g, '_');
  const encoded = encodeURIComponent(name).replace(/['()*]/g, char => `%${char.charCodeAt(0).toString(16).toUpperCase()}`);
  return `${inline ? 'inline' : 'attachment'}; filename="${fallback}"; filename*=UTF-8''${encoded}`;
}

export async function previewMime(filePath, name, declared) {
  const handle = await open(filePath, 'r');
  try {
    const bytes = Buffer.alloc(64);
    const { bytesRead } = await handle.read(bytes, 0, bytes.length, 0);
    const data = bytes.subarray(0, bytesRead);
    const ascii = data.toString('latin1');
    if (data.subarray(0, 8).equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]))) return 'image/png';
    if (data.length > 3 && data[0] === 255 && data[1] === 216 && data[2] === 255) return 'image/jpeg';
    if (/^GIF8[79]a/.test(ascii)) return 'image/gif';
    if (ascii.startsWith('RIFF') && ascii.slice(8, 12) === 'WEBP') return 'image/webp';
    if (ascii.slice(4, 8) === 'ftyp' && /avif|avis/.test(ascii.slice(8, 32))) return 'image/avif';
    if (ascii.startsWith('RIFF') && ascii.slice(8, 12) === 'WAVE') return 'audio/wav';
    if (ascii.startsWith('fLaC')) return 'audio/flac';
    if (ascii.slice(4, 8) === 'ftyp' && /^(isom|iso[2-9]|mp4[12]|avc1|M4V |MSNV|qt  )$/.test(ascii.slice(8, 12))) return 'video/mp4';
    if (ascii.slice(4, 8) === 'ftyp' && /^M4A /.test(ascii.slice(8, 12))) return 'audio/mp4';
    if (data.subarray(0, 4).equals(Buffer.from([26, 69, 223, 163])) && /\.webm$/i.test(name)) return 'video/webm';
    if (ascii.startsWith('OggS') && /\.og[agv]$/i.test(name)) return declared === 'video/ogg' ? 'video/ogg' : 'audio/ogg';
    if (/\.mp3$/i.test(name) && (ascii.startsWith('ID3') || (data[0] === 255 && (data[1] & 224) === 224))) return 'audio/mpeg';
    return null;
  } finally { await handle.close(); }
}

export async function findAnalyzer(candidate) {
  if (!candidate || typeof candidate !== 'string') return null;
  const fullPath = path.resolve(candidate);
  // Shell wrappers are deliberately not run: uploaded filenames never become shell code.
  if (process.platform === 'win32' && !/\.exe$/i.test(fullPath)) return null;
  try {
    await access(fullPath, process.platform === 'win32' ? constants.F_OK : constants.X_OK);
    if (!(await stat(fullPath)).isFile()) return null;
    return fullPath;
  } catch { return null; }
}

export async function inspectApk(analyzer, filePath) {
  if (!analyzer) return { note: '未配置可直接执行的 Android SDK apkanalyzer，包名、版本、签名与兼容性未知。' };
  const run = async args => {
    const result = await execFileAsync(analyzer, [...args, filePath], {
      shell: false, timeout: 5000, maxBuffer: 256 * 1024, windowsHide: true,
      encoding: 'utf8'
    });
    return result.stdout.trim();
  };
  try {
    const summary = await run(['apk', 'summary']);
    const match = /^([A-Za-z_][A-Za-z\d_.]*)\s+(\d+)\s+(.{1,200})$/.exec(summary);
    if (!match) return { note: 'APK 元数据解析失败；未确认此文件可安装。' };
    const apk = { packageName: match[1], versionCode: Number(match[2]), versionName: match[3], note: '已读取 APK 清单；签名、安装结果与真机兼容性尚未验证。' };
    for (const [command, key] of [['min-sdk', 'minSdk'], ['target-sdk', 'targetSdk']]) {
      try { const value = await run(['manifest', command]); if (/^\d+$/.test(value)) apk[key] = Number(value); }
      catch { /* Optional metadata remains unknown rather than guessed. */ }
    }
    return apk;
  } catch { return { note: 'APK 解析不可用或超时；包名、版本与兼容性未知。' }; }
}

export function mdLabel(value) {
  return String(value ?? '').replace(/[\x00-\x1f\x7f]/g, ' ').replace(/[\\`*_[\]{}()<>#!|~]/g, '\\$&');
}

export function fencedText(value) {
  const matches = String(value).match(/`+/g) || [];
  const fence = '`'.repeat(Math.max(3, ...matches.map(part => part.length + 1)));
  return `${fence}text\n${value}\n${fence}`;
}
