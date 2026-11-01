#!/usr/bin/env node
import path from 'node:path';
import os from 'node:os';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { isIP } from 'node:net';
import { createRelayServer } from './server.mjs';
import { VERSION } from './store.mjs';

function envInteger(name, fallback, minimum = 0) {
  const value = process.env[name];
  if (value === undefined || value === '') return fallback;
  if (!/^\d+$/.test(value) || !Number.isSafeInteger(Number(value)) || Number(value) < minimum) {
    throw new Error(`${name} 必须是大于或等于 ${minimum} 的整数。`);
  }
  return Number(value);
}

export function parseCliArgs(args = process.argv.slice(2)) {
  let values;
  try {
    ({ values } = parseArgs({ args, strict: true, allowPositionals: false, options: {
      host: { type: 'string' }, port: { type: 'string' }, strictPort: { type: 'boolean' },
      help: { type: 'boolean', short: 'h' }, version: { type: 'boolean', short: 'v' }
    } }));
  } catch (error) { throw new Error(`命令行参数无效：${error.message}。运行 --help 查看用法。`); }
  const result = { help: Boolean(values.help), version: Boolean(values.version), strictPort: Boolean(values.strictPort) };
  if (values.host !== undefined) {
    let host = values.host;
    if (host.startsWith('[') && host.endsWith(']') && isIP(host.slice(1, -1)) === 6) host = host.slice(1, -1);
    const hostname = /^(?=.{1,253}$)[a-z\d](?:[a-z\d-]{0,61}[a-z\d])?(?:\.[a-z\d](?:[a-z\d-]{0,61}[a-z\d])?)*\.?$/i;
    if (!isIP(host) && !hostname.test(host)) throw new Error('--host 必须是有效的 IP 地址或主机名，不能包含协议、路径或空白。');
    result.host = host;
  }
  if (values.port !== undefined) {
    if (!/^\d+$/.test(values.port) || !Number.isSafeInteger(Number(values.port)) || Number(values.port) > 65535) {
      throw new Error('--port 必须是 0 至 65535 之间的整数。');
    }
    result.port = Number(values.port);
  }
  return result;
}

export async function main() {
  const arguments_ = parseCliArgs();
  if (arguments_.help) {
    console.log(`随手传 ${VERSION}\n\n运行：npm start -- [参数]\n示例：npm run dev -- --host 0.0.0.0 --port 4318 --strictPort\n\n命令行参数（优先于环境变量）：\n  --host <地址>             监听 IP 地址或主机名\n  --port <端口>             0 至 65535；0 由系统分配临时端口\n  --strictPort              端口占用即失败（本程序始终如此，不自动换端口）\n  --help, -h                查看帮助\n  --version, -v             查看版本\n\n环境变量（亦可写入工作目录的 .env）：\n  LAN_RELAY_HOST             监听地址，默认 0.0.0.0\n  LAN_RELAY_PORT             端口，默认 PORT 或 4318\n  LAN_RELAY_DATA_DIR         数据目录，默认项目内 .lan-relay-data\n  LAN_RELAY_MAX_FILE_BYTES   单文件限制，默认 2 GiB\n  LAN_RELAY_MAX_STORAGE_BYTES 收件箱配额，默认 20 GiB\n  LAN_RELAY_CHUNK_SIZE       分片字节数，默认 2 MiB\n  LAN_RELAY_ALLOWED_ORIGINS  精确可信 origin，逗号分隔，仅受控预览需要\n  LAN_RELAY_TLS_KEY          TLS 私钥文件路径\n  LAN_RELAY_TLS_CERT         TLS 证书文件路径（必须与私钥同时配置）\n  LAN_RELAY_APKANALYZER      可选、可直接执行的 Android SDK apkanalyzer 路径\n\n不会建立公网隧道，不会自动安装 APK。Ctrl+C 安全停止服务。`);
    return;
  }
  if (arguments_.version) { console.log(VERSION); return; }
  try { process.loadEnvFile(path.resolve('.env')); }
  catch (error) { if (error.code !== 'ENOENT') throw new Error(`无法读取 .env 配置：${error.code || '格式错误'}`); }
  const keyPath = process.env.LAN_RELAY_TLS_KEY;
  const certPath = process.env.LAN_RELAY_TLS_CERT;
  if (Boolean(keyPath) !== Boolean(certPath)) throw new Error('TLS 私钥和证书必须同时配置。');
  const tls = keyPath ? { key: await readFile(path.resolve(keyPath)), cert: await readFile(path.resolve(certPath)) } : undefined;
  const port = arguments_.port ?? envInteger('LAN_RELAY_PORT', undefined) ?? envInteger('PORT', 4318);
  if (port > 65535) throw new Error('端口必须在 0 至 65535 之间。');
  const host = arguments_.host ?? (process.env.LAN_RELAY_HOST || '0.0.0.0');
  const app = await createRelayServer({
    dataDir: process.env.LAN_RELAY_DATA_DIR,
    maxFileBytes: envInteger('LAN_RELAY_MAX_FILE_BYTES', 2 * 1024 ** 3),
    maxStorageBytes: envInteger('LAN_RELAY_MAX_STORAGE_BYTES', 20 * 1024 ** 3),
    chunkSize: envInteger('LAN_RELAY_CHUNK_SIZE', 2 * 1024 ** 2, 1),
    allowedOrigins: (process.env.LAN_RELAY_ALLOWED_ORIGINS || '').split(',').map(value => value.trim()).filter(Boolean),
    apkAnalyzerPath: process.env.LAN_RELAY_APKANALYZER,
    tls
  });
  try {
    const address = await app.listen(port, host);
    const scheme = tls ? 'https' : 'http';
    const localHost = host === '0.0.0.0' ? '127.0.0.1' : host === '::' ? '::1' : host;
    const bracket = value => value.includes(':') ? `[${value}]` : value;
    console.log(`\n随手传 ${VERSION} 已启动\n`);
    console.log(`仅在自己的电脑打开此管理者链接：\n${scheme}://${bracket(localHost)}:${address.port}/#owner=${app.ownerSecret}\n`);
    console.log('手机请先与电脑连接到可互访的局域网，再扫描网页内的配对二维码。');
    const addresses = new Set();
    if (host === '0.0.0.0' || host === '::') {
      for (const entries of Object.values(os.networkInterfaces())) {
        for (const entry of entries || []) if (!entry.internal && entry.family === 'IPv4') addresses.add(entry.address);
      }
    } else if (!['127.0.0.1', '::1', 'localhost'].includes(host)) addresses.add(host);
    for (const networkAddress of addresses) console.log(`局域网入口：${scheme}://${bracket(networkAddress)}:${address.port}/`);
    console.log(tls ? '\n已启用 HTTPS；手机仍需信任所配置的证书。' : '\n当前为 HTTP 局域网模式：传输未加密，剪贴板增强和 PWA 能力可能不可用。');
    console.log('管理者链接包含本机密钥，请勿转发。保持此进程运行，Ctrl+C 停止。\n');
  } catch (error) { await app.close(); throw error; }
  let stopping = false;
  const stop = async () => {
    if (stopping) return;
    stopping = true;
    try { await app.close(); console.log('随手传已停止；文件和未完成传输记录已保留。'); }
    catch { console.error('停止服务时遇到问题，请检查本机文件状态。'); process.exitCode = 1; }
  };
  process.once('SIGINT', stop);
  process.once('SIGTERM', stop);
  return app;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch(error => {
    console.error(`随手传启动失败：${error.code === 'EADDRINUSE' ? '端口已被占用，请更换 LAN_RELAY_PORT。' : error.message}`);
    process.exitCode = 1;
  });
}
