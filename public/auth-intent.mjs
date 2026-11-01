// Authentication links stay in the URL fragment and are removed before network I/O.
const SECRET = /^[A-Za-z0-9_-]{43}$/;
const CODE = /^\d{6}$/;

export function readAuthIntent(hash = '') {
  const fields = new URLSearchParams(String(hash).replace(/^#/, ''));
  if (!['owner', 'pair', 'code'].some((key) => fields.has(key))) return null;
  const invalid = () => ({ kind: 'invalid', message: '连接链接不完整或格式不正确，请重新获取电脑上的连接链接。' });
  if (['owner', 'pair', 'code'].some((key) => fields.getAll(key).length > 1)) return invalid();
  const owner = fields.get('owner');
  const token = fields.get('pair');
  const code = fields.get('code');
  if (owner !== null) return SECRET.test(owner) && token === null && code === null ? { kind: 'owner', secret: owner } : invalid();
  if (token !== null) return SECRET.test(token) && (code === null || CODE.test(code)) ? { kind: 'pair', token, code: code || '' } : invalid();
  return CODE.test(code || '') ? { kind: 'code', code } : invalid();
}

export function consumeAuthIntent(location, history) {
  const intent = readAuthIntent(location.hash);
  if (intent) history.replaceState(null, '', `${location.pathname}${location.search}`);
  return intent;
}

export function pairingLink(address, { token, code }) {
  if (!SECRET.test(token) || !CODE.test(code)) throw new Error('配对信息无效。');
  const url = new URL(address);
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password) throw new Error('连接地址无效。');
  url.hash = new URLSearchParams({ pair: token, code }).toString();
  return url.href;
}

export function defaultDeviceName(saved) {
  return typeof saved === 'string' && saved.trim() ? Array.from(saved.trim()).slice(0, 60).join('') : '我的手机';
}

// Serialize cookie-changing authentication requests. A newer link invalidates older
// UI results immediately, without starting a second login request in parallel.
export function createAuthIntentQueue(handle) {
  let latest = 0;
  let tail = Promise.resolve();
  return (intent) => {
    const revision = ++latest;
    const isCurrent = () => revision === latest;
    const task = tail.catch(() => {}).then(() => isCurrent() ? handle(intent, isCurrent) : undefined);
    tail = task;
    return task;
  };
}
