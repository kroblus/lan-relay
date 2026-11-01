// Private stdio host for the Windows launcher. Never run with inherited stdout:
// the ready message contains the local owner's bootstrap link.
import path from 'node:path';
import os from 'node:os';
import readline from 'node:readline';
import { fileURLToPath } from 'node:url';
import { createRelayServer } from '../../src/server.mjs';

const appRoot = fileURLToPath(new URL('../../', import.meta.url));
const defaultData = path.join(process.env.LOCALAPPDATA || path.join(os.homedir(), '.local', 'share'), 'LanRelay', 'data');
const dataDir = path.resolve(process.env.LAN_RELAY_DATA_DIR || defaultData);
let app;
let stopping = false;
const send = message => process.stdout.write(`${JSON.stringify(message)}\n`);

async function stop() {
  if (stopping) return;
  stopping = true;
  try { await app?.close(); send({ event: 'stopped' }); }
  finally { process.exit(0); }
}

try {
  if (process.stdin.isTTY || process.stdout.isTTY) throw Object.assign(new Error(), { code: 'PRIVATE_CHANNEL_REQUIRED' });
  const rawPort = process.env.LAN_RELAY_PORT || '4318';
  if (!/^\d+$/.test(rawPort) || Number(rawPort) > 65535) throw Object.assign(new Error(), { code: 'INVALID_PORT' });
  app = await createRelayServer({ dataDir, publicDir: path.join(appRoot, 'public') });
  const address = await app.listen(Number(rawPort), '0.0.0.0');
  const input = readline.createInterface({ input: process.stdin, terminal: false });
  input.on('line', line => { if (line === 'shutdown') void stop(); });
  input.on('close', () => void stop());
  process.once('SIGTERM', () => void stop());
  process.once('SIGINT', () => void stop());
  send({ event: 'ready', url: `http://127.0.0.1:${address.port}/#owner=${app.ownerSecret}`, port: address.port, pid: process.pid, dataDir });
} catch (error) {
  await app?.close().catch(() => {});
  const messages = {
    EADDRINUSE: '端口已被占用。请先退出之前运行的随手传窗口，再打开桌面应用。',
    EACCES: '没有权限读取或写入数据目录，请检查所选择的目录权限。',
    EPERM: '系统阻止了本地服务启动，请检查应用和网络权限。',
    INVALID_PORT: 'LAN_RELAY_PORT 必须是 0 至 65535 的整数。',
    PRIVATE_CHANNEL_REQUIRED: '这个入口由桌面启动器使用，请运行 LanRelay.exe。'
  };
  send({ event: 'error', code: error.code || 'START_FAILED', message: messages[error.code] || '本地服务未能启动。请确认没有另一个随手传实例使用相同的数据目录。' });
  process.exitCode = 1;
}
