import { qrSvg } from './qr.mjs';

const $ = (id) => document.getElementById(id);
const state = {
  device: null, csrf: '', info: null, items: [], devices: [], requests: [],
  filter: 'all', query: '', mode: 'text', currentBuild: null, attachments: new Set(),
  feedLoaded: false, hasMore: false, loadingMore: false, pairing: null, pairingUrl: '',
  pairingTimer: null, pairingRequest: null, claimTimer: null, sse: null, refreshTimer: null,
  refreshing: false, refreshAgain: false, sessionCheck: false, seenRequests: new Set(),
  jobs: [], activeJobs: 0, queueNodes: new Map(), renderQueueTimer: null, resumeTarget: null,
  sending: false, attachmentDraft: new Set(), connectionBusy: false, lastConnection: 'connecting',
};
const FILTERS = [
  { id: 'all', label: '所有传递', icon: '⊞', match: () => true },
  { id: 'files', label: '文件', icon: '▤', match: (item) => item.kind === 'file' },
  { id: 'images', label: '图片', icon: '▧', match: (item) => item.kind === 'file' && /^image\//.test(item.mime || '') },
  { id: 'apk', label: '安装包', icon: '◇', match: (item) => item.kind === 'file' && item.isApk },
  { id: 'text', label: '文字', icon: '≡', match: (item) => item.kind === 'text' },
  { id: 'feedback', label: '测试反馈', icon: '☷', match: (item) => item.kind === 'feedback' },
];
const TEST_STATUSES = [
  ['not-tested', '尚未标记测试'],
  ['user-confirmed-installed', '用户确认已安装'],
  ['issue-found', '用户标记发现问题'],
  ['user-confirmed-passed', '用户确认测试通过'],
];
const ACTIVE_STATES = new Set(['queued', 'hashing', 'uploading', 'completing']);
const FINISHED_STATES = new Set(['complete', 'canceled']);
const IMAGE_MIMES = new Set(['image/jpeg', 'image/png', 'image/webp', 'image/gif', 'image/avif', 'image/bmp']);
let joinToken = '';
let confirmResolve = null;
let dragDepth = 0;

function node(tag, className = '', text = '') {
  const element = document.createElement(tag);
  if (className) element.className = className;
  if (text !== '') element.textContent = String(text);
  return element;
}
function button(text, className, action) {
  const element = node('button', className, text);
  element.type = 'button';
  if (action) element.addEventListener('click', action);
  return element;
}
function textLink(text, href, className = '') {
  const element = node('a', className, text);
  element.href = href;
  return element;
}
function setError(id, message = '') {
  const element = $(id);
  element.textContent = message;
  element.hidden = !message;
}
function toast(message, error = false) {
  const element = node('div', `toast${error ? ' error' : ''}`, message);
  $('toastRegion').append(element);
  setTimeout(() => element.remove(), error ? 6500 : 3800);
}
function formatBytes(size) {
  if (!Number.isFinite(Number(size))) return '大小未知';
  const value = Number(size);
  if (value < 1024) return `${value} B`;
  const units = ['KiB', 'MiB', 'GiB', 'TiB'];
  const power = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length);
  return `${(value / 1024 ** power).toFixed(value / 1024 ** power < 10 ? 1 : 0)} ${units[power - 1]}`;
}
function readableDate(value, timeOnly = false) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return '时间未知';
  return new Intl.DateTimeFormat('zh-CN', timeOnly ? { hour: '2-digit', minute: '2-digit' } : {
    month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit',
  }).format(date);
}
function dateGroup(value) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return '较早的记录';
  const today = new Date();
  const yesterday = new Date(); yesterday.setDate(today.getDate() - 1);
  if (date.toDateString() === today.toDateString()) return '今天';
  if (date.toDateString() === yesterday.toDateString()) return '昨天';
  return new Intl.DateTimeFormat('zh-CN', { year: 'numeric', month: 'long', day: 'numeric' }).format(date);
}
function storageGet(storage, key, fallback = null) {
  try { const value = storage.getItem(key); return value ? JSON.parse(value) : fallback; } catch { return fallback; }
}
function storageSet(storage, key, value) {
  try { if (value === null) storage.removeItem(key); else storage.setItem(key, JSON.stringify(value)); } catch { /* Private modes may not allow storage. */ }
}
function deviceKey(suffix) { return `lan-relay.${state.device?.id || 'guest'}.${suffix}`; }
function fileUrl(id, preview = false) { return `/api/files/${encodeURIComponent(id)}${preview ? '/preview' : ''}`; }
function itemById(id) { return state.items.find((item) => item.id === id); }
function safeHttpUrl(value) {
  try { const url = new URL(String(value).trim()); return ['http:', 'https:'].includes(url.protocol) ? url.href : null; } catch { return null; }
}
async function api(path, options = {}) {
  const { method = 'GET', body, headers: extraHeaders = {}, signal, auth = true, csrfRetried = false } = options;
  const headers = { ...extraHeaders };
  let payload = body;
  if (body !== undefined && !(body instanceof Blob) && !(body instanceof ArrayBuffer) && !ArrayBuffer.isView(body)) {
    payload = JSON.stringify(body);
    headers['Content-Type'] = 'application/json';
  }
  if (method !== 'GET' && method !== 'HEAD' && auth) {
    if (!state.csrf) throw new Error('连接已失效，请重新连接。');
    headers['X-CSRF-Token'] = state.csrf;
  }
  let response;
  try {
    response = await fetch(path, { method, headers, body: payload, signal, credentials: 'same-origin', cache: 'no-store' });
  } catch (error) {
    if (error.name === 'AbortError') throw error;
    throw new Error('暂时连不上电脑。请检查 Wi-Fi、电脑是否开机，然后重试。');
  }
  let data;
  try { data = await response.json(); } catch { data = null; }
  if (!response.ok) {
    if (response.status === 403 && data?.error?.code === 'CSRF_REQUIRED' && state.device && !csrfRetried) {
      const expectedDevice = state.device;
      const session = await api('/api/session', { auth: false });
      if (session.authenticated && session.device?.id === expectedDevice.id && session.device?.role === expectedDevice.role) {
        state.csrf = session.csrfToken;
        return api(path, { ...options, csrfRetried: true });
      }
      disconnectUI('此浏览器的登录身份已变化，请刷新页面重新进入。');
      throw new Error('登录身份已变化，本次操作没有继续执行。');
    }
    const error = new Error(data?.error?.message || `请求未完成（${response.status}），请重试。`);
    error.status = response.status;
    error.code = data?.error?.code;
    error.details = data?.error?.details;
    if (response.status === 401 && state.device && path !== '/api/session') void verifySession();
    throw error;
  }
  return data || {};
}
async function busyAction(element, action) {
  element.disabled = true;
  try { return await action(); } catch (error) { toast(error.message || '操作未完成，请重试。', true); return null; }
  finally { element.disabled = false; }
}
function openDialog(id) { const dialog = $(id); if (!dialog.open) dialog.showModal(); }
function closeDialog(dialog) { if (dialog.open) dialog.close(); }
async function copyText(text) {
  const value = String(text ?? '');
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(value);
      toast('已复制');
      return;
    }
  } catch { /* Use a user-initiated fallback on local HTTP. */ }
  const temporary = node('textarea');
  temporary.value = value;
  temporary.style.cssText = 'position:fixed;left:-9999px;top:0;opacity:0';
  const activeDialog = document.querySelector('dialog[open]');
  (activeDialog || document.body).append(temporary);
  const focused = document.activeElement;
  temporary.focus(); temporary.select(); temporary.setSelectionRange(0, value.length);
  let copied = false;
  try { copied = document.execCommand('copy'); } catch { /* Manual selection remains available. */ }
  temporary.remove();
  focused?.focus({ preventScroll: true });
  if (copied) { toast('已复制'); return; }
  $('copyFallback').value = value;
  openDialog('copyDialog');
  requestAnimationFrame(() => { $('copyFallback').focus(); $('copyFallback').select(); });
}
function confirmAction(title, message, label = '确认') {
  if (confirmResolve) confirmResolve(false);
  $('confirmTitle').textContent = title;
  $('confirmMessage').textContent = message;
  $('confirmAccept').textContent = label;
  openDialog('confirmDialog');
  return new Promise((resolve) => { confirmResolve = resolve; });
}

function showJoin(message = '') {
  state.device = null; state.csrf = '';
  $('appShell').hidden = true;
  $('welcome').hidden = false;
  $('authLoading').hidden = true;
  $('pendingPanel').hidden = true;
  $('joinForm').hidden = false;
  $('pairCodeField').hidden = Boolean(joinToken);
  $('pairCode').required = !joinToken;
  $('tokenNotice').hidden = !joinToken;
  setError('authError', message);
}
async function initialize() {
  const fragment = new URLSearchParams(location.hash.slice(1));
  const ownerSecret = fragment.get('owner');
  joinToken = fragment.get('pair') || '';
  if (ownerSecret || joinToken) history.replaceState(null, '', `${location.pathname}${location.search}`);
  const savedName = storageGet(localStorage, 'lan-relay.device-name', '');
  $('deviceName').value = typeof savedName === 'string' ? savedName : '';
  try {
    let session;
    if (ownerSecret) session = await api('/api/session/owner', { method: 'POST', body: { secret: ownerSecret }, auth: false });
    else session = await api('/api/session', { auth: false });
    if (session.device && session.csrfToken) { await enterApp(session); return; }
    const pending = storageGet(sessionStorage, 'lan-relay.pending');
    if (!joinToken && pending?.id && pending?.claimToken) {
      state.pairingRequest = pending; showPending(); void pollClaim(); return;
    }
    showJoin();
  } catch (error) { showJoin(error.message); }
}
async function submitJoin(event) {
  event.preventDefault();
  const name = $('deviceName').value.trim();
  const code = $('pairCode').value.trim();
  if (!name) { setError('authError', '先给这台设备起个名字。'); $('deviceName').focus(); return; }
  if (!joinToken && !/^\d{6}$/.test(code)) { setError('authError', '请输入电脑页面上的六位连接码。'); return; }
  $('joinButton').disabled = true; setError('authError');
  try {
    const result = await api('/api/pair/requests', { method: 'POST', body: { name, ...(joinToken ? { token: joinToken } : { code }) }, auth: false });
    state.pairingRequest = { id: result.id, claimToken: result.claimToken, name };
    storageSet(sessionStorage, 'lan-relay.pending', state.pairingRequest);
    storageSet(localStorage, 'lan-relay.device-name', name);
    joinToken = ''; showPending(); void pollClaim();
  } catch (error) {
    if (joinToken && ['INVALID_PAIRING', 'PAIRING_EXPIRED', 'PAIRING_INVALID'].includes(error.code)) { joinToken = ''; showJoin(error.message); }
    else setError('authError', error.message);
  }
  finally { $('joinButton').disabled = false; }
}
function showPending(message = '') {
  $('authLoading').hidden = true; $('joinForm').hidden = true; $('pendingPanel').hidden = false;
  $('pendingCopy').textContent = message || `「${state.pairingRequest?.name || '这台设备'}」已申请加入。请在电脑的「连接设备」里确认。`;
}
async function pollClaim() {
  clearTimeout(state.claimTimer);
  const request = state.pairingRequest;
  if (!request) return;
  try {
    const result = await api('/api/pair/claim', { method: 'POST', body: { id: request.id, claimToken: request.claimToken }, auth: false });
    if (state.pairingRequest !== request) return;
    if (result.status === 'approved' && result.device) {
      storageSet(sessionStorage, 'lan-relay.pending', null); state.pairingRequest = null;
      await enterApp(result); toast('连接好了，可以开始传递了'); return;
    }
    if (result.status === 'rejected' || result.status === 'expired') {
      storageSet(sessionStorage, 'lan-relay.pending', null); state.pairingRequest = null;
      showJoin(result.status === 'rejected' ? '电脑未同意这次连接。可以重新申请。' : '这次连接申请已过期，请获取新的连接码。'); return;
    }
    showPending();
  } catch (error) {
    if (state.pairingRequest !== request) return;
    // A response can be lost after the claim set its cookie. Recover without consuming twice.
    try {
      const session = await api('/api/session', { auth: false });
      if (session.authenticated && session.device) {
        storageSet(sessionStorage, 'lan-relay.pending', null); state.pairingRequest = null;
        await enterApp(session); return;
      }
    } catch { /* Keep the pending claim; connectivity can recover. */ }
    showPending(error.message);
  }
  if (state.pairingRequest === request) state.claimTimer = setTimeout(pollClaim, 2200);
}
async function enterApp(session) {
  state.device = session.device; state.csrf = session.csrfToken;
  state.items = []; state.devices = []; state.requests = []; state.seenRequests.clear();
  state.feedLoaded = false; state.filter = 'all'; state.query = ''; $('searchInput').value = '';
  state.jobs = []; state.queueNodes.clear(); $('queueList').replaceChildren();
  $('welcome').hidden = true; $('appShell').hidden = false;
  $('ownDeviceName').textContent = state.device.name;
  $('ownDeviceRole').textContent = state.device.role === 'owner' ? '此浏览器 · 管理设备' : '此浏览器 · 已配对';
  $('connectButton').children[1].firstChild.textContent = state.device.role === 'owner' ? '连接设备' : '设备与连接';
  $('connectButton').querySelector('small').textContent = state.device.role === 'owner' ? '让另一台设备加入' : '查看已配对设备';
  state.currentBuild = storageGet(localStorage, deviceKey('current-build'));
  if (!state.currentBuild?.id) state.currentBuild = null;
  const draft = storageGet(sessionStorage, deviceKey('draft'), {});
  $('composerText').value = typeof draft.text === 'string' ? draft.text : '';
  $('projectInput').value = typeof draft.project === 'string' ? draft.project : '';
  state.mode = draft.mode === 'feedback' ? 'feedback' : 'text';
  state.attachments = new Set(Array.isArray(draft.attachments) ? draft.attachments : []);
  renderFilters(); renderCurrentBuild(); renderComposerMode(); renderSelectedAttachments();
  const results = await Promise.allSettled([loadItems(true), loadInfo(), loadDevices(), restoreQueue(), loadRequests()]);
  for (const result of results) if (result.status === 'rejected') toast(result.reason.message, true);
  connectEvents();
}
function persistDraft() {
  if (!state.device) return;
  storageSet(sessionStorage, deviceKey('draft'), {
    text: $('composerText').value, project: $('projectInput').value, mode: state.mode, attachments: [...state.attachments],
  });
}
async function verifySession() {
  if (state.sessionCheck || !state.device) return;
  state.sessionCheck = true;
  try {
    const session = await api('/api/session', { auth: false });
    if (!session.authenticated || !session.device) disconnectUI('此设备的连接已失效，可能已被撤销。请重新申请连接。');
    else if (session.device.id !== state.device.id || session.device.role !== state.device.role) disconnectUI('此浏览器的登录身份已变化，请刷新页面重新进入。');
    else state.csrf = session.csrfToken;
  } catch { setConnectionStatus('offline'); }
  finally { state.sessionCheck = false; }
}
function disconnectUI(message = '') {
  persistDraft(); persistQueue();
  state.sse?.close(); state.sse = null;
  clearTimeout(state.refreshTimer); clearInterval(state.pairingTimer);
  for (const job of state.jobs) { job.stopped = true; job.controller?.abort(); job.cancelHash?.(); }
  for (const dialog of document.querySelectorAll('dialog[open]')) dialog.close();
  state.pairing = null; state.currentBuild = null; state.items = []; state.jobs = [];
  showJoin(message);
}
function setConnectionStatus(status) {
  state.lastConnection = status;
  $('connectionDot').className = `status-dot${status === 'connected' ? '' : status === 'connecting' ? ' connecting' : ' offline'}`;
  $('connectionLabel').textContent = { connected: '与电脑保持连接', connecting: '正在重连…', offline: '连接暂时中断' }[status];
}
function connectEvents() {
  state.sse?.close();
  if (!state.device) return;
  setConnectionStatus('connecting');
  const source = new EventSource('/api/events'); state.sse = source;
  source.addEventListener('open', () => { if (state.sse !== source) return; setConnectionStatus('connected'); scheduleRefresh(); });
  source.addEventListener('change', () => scheduleRefresh());
  source.addEventListener('error', () => {
    if (state.sse !== source) return;
    setConnectionStatus(navigator.onLine === false ? 'offline' : 'connecting');
    void verifySession();
  });
}
function scheduleRefresh() {
  clearTimeout(state.refreshTimer);
  state.refreshTimer = setTimeout(refreshState, 250);
}
async function refreshState() {
  if (!state.device) return;
  if (state.refreshing) { state.refreshAgain = true; return; }
  state.refreshing = true;
  do {
    state.refreshAgain = false;
    const results = await Promise.allSettled([loadItems(), loadInfo(), loadDevices(), loadRequests()]);
    for (const result of results) {
      if (result.status === 'rejected' && result.reason.status === 401) void verifySession();
    }
  } while (state.refreshAgain && state.device);
  state.refreshing = false;
}
async function loadInfo() {
  if (!state.device) return;
  const result = await api('/api/info'); state.info = result;
  $('storageSummary').hidden = false;
  $('storageText').textContent = `${formatBytes(result.usedBytes)} / ${formatBytes(result.maxStorageBytes)}`;
  $('storageFill').style.width = `${Math.min(100, Math.max(0, (result.usedBytes / result.maxStorageBytes) * 100))}%`;
  if ($('connectionDialog').open) renderConnectionFacts();
}
async function loadDevices() {
  if (!state.device) return;
  const result = await api('/api/devices'); state.devices = result.devices || [];
  if ($('connectionDialog').open) renderDevices();
}
async function loadRequests() {
  if (state.device?.role !== 'owner') return;
  const result = await api('/api/pair/requests');
  state.requests = (result.requests || []).filter((request) => request.status === 'pending');
  for (const request of state.requests) {
    if (!state.seenRequests.has(request.id)) { state.seenRequests.add(request.id); toast(`「${request.name}」请求连接，请在「连接设备」中确认。`); }
  }
  $('connectButton').querySelector('small').textContent = state.requests.length ? `${state.requests.length} 台设备等待确认` : '让另一台设备加入';
  if ($('connectionDialog').open) renderRequests();
}
async function loadItems(initial = false) {
  try {
    const result = await api('/api/items?limit=200');
    const incoming = result.items || [];
    if (initial) { state.items = incoming; state.hasMore = incoming.length === 200; }
    else {
      const merged = new Map(state.items.map((item) => [item.id, item]));
      for (const item of incoming) merged.set(item.id, item);
      state.items = [...merged.values()].sort((a, b) => b.createdAt.localeCompare(a.createdAt) || b.id.localeCompare(a.id));
      if (!state.feedLoaded) state.hasMore = incoming.length === 200;
    }
    state.feedLoaded = true;
    $('feed').setAttribute('aria-busy', 'false'); $('feedError').hidden = true;
    renderFeed(); renderFilters(); renderProjectSuggestions(); renderSelectedAttachments();
    if (state.currentBuild) {
      const current = itemById(state.currentBuild.id);
      if (current?.isApk) { state.currentBuild = current; renderCurrentBuild(); }
    }
  } catch (error) {
    $('feed').setAttribute('aria-busy', 'false');
    $('feedErrorText').textContent = error.message; $('feedError').hidden = false;
    if (!state.feedLoaded) $('feed').replaceChildren(node('div', 'empty-inline', '记录暂时未能加载，已有内容没有被清除。'));
    throw error;
  }
}
async function loadMore() {
  if (state.loadingMore || !state.items.length) return;
  state.loadingMore = true; $('loadMore').disabled = true;
  try {
    const oldest = state.items[state.items.length - 1];
    const cursor = new URLSearchParams({ limit: '200', before: oldest.createdAt, beforeId: oldest.id });
    const result = await api(`/api/items?${cursor}`);
    const merged = new Map(state.items.map((item) => [item.id, item]));
    for (const item of result.items || []) merged.set(item.id, item);
    state.items = [...merged.values()].sort((a, b) => b.createdAt.localeCompare(a.createdAt) || b.id.localeCompare(a.id));
    state.hasMore = (result.items || []).length === 200;
    renderFeed(); renderFilters(); renderProjectSuggestions();
  } catch (error) { toast(error.message, true); }
  finally { state.loadingMore = false; $('loadMore').disabled = false; }
}
function renderFilters() {
  const fragment = document.createDocumentFragment();
  for (const filter of FILTERS) {
    const control = button('', `filter-button${state.filter === filter.id ? ' active' : ''}`, () => {
      state.filter = filter.id; renderFilters(); renderFeed();
    });
    control.dataset.filter = filter.id;
    if (state.filter === filter.id) control.setAttribute('aria-current', 'page');
    control.append(node('span', 'filter-icon', filter.icon), node('span', '', filter.label), node('span', 'filter-count', state.items.filter(filter.match).length));
    fragment.append(control);
  }
  $('filters').replaceChildren(fragment);
  const filter = FILTERS.find((entry) => entry.id === state.filter) || FILTERS[0];
  $('feedTitle').firstChild.textContent = filter.label;
}
function renderProjectSuggestions() {
  const projects = [...new Set(state.items.map((item) => item.project).filter(Boolean))].sort();
  $('projectSuggestions').replaceChildren(...projects.map((project) => { const option = node('option'); option.value = project; return option; }));
}
function renderFeed() {
  if (!state.feedLoaded) return;
  const filter = FILTERS.find((entry) => entry.id === state.filter) || FILTERS[0];
  const query = state.query.trim().toLocaleLowerCase();
  const items = state.items.filter((item) => filter.match(item) && (!query || [
    item.name, item.text, item.note, item.project, item.deviceName, item.sha256,
    item.apk?.packageName, item.apk?.versionName, item.apk?.label,
  ].filter(Boolean).join('\n').toLocaleLowerCase().includes(query)));
  $('feedCount').textContent = items.length ? String(items.length) : '';
  $('feedFooter').hidden = !state.hasMore;
  if (!items.length) {
    const empty = node('div', 'empty-state');
    empty.append(node('div', 'empty-art', state.items.length ? '⌕' : '↗'));
    empty.append(node('h2', '', state.items.length ? '这里暂时没有匹配的内容' : '第一份内容，从你开始'));
    empty.append(node('p', '', state.items.length ? '换个关键词或筛选，继续找找。' : '写一段文字，或拖进一个文件。\n它会出现在已配对设备的同一条内容流里。'));
    if (!state.items.length) empty.append(button('选择一个文件', 'button quiet', () => $('fileInput').click()));
    else empty.append(button('查看所有传递', 'button quiet small', () => { state.filter = 'all'; state.query = ''; $('searchInput').value = ''; renderFilters(); renderFeed(); }));
    $('feed').replaceChildren(empty); return;
  }
  const fragment = document.createDocumentFragment();
  let day = '';
  for (const item of items) {
    const nextDay = dateGroup(item.createdAt);
    if (nextDay !== day) { fragment.append(node('div', 'day-label', nextDay)); day = nextDay; }
    fragment.append(renderItem(item));
  }
  $('feed').replaceChildren(fragment);
}
function renderItem(item) {
  const article = node('article', `item${item.isApk ? ' apk-item' : ''}${item.kind === 'feedback' ? ' feedback-item' : ''}`);
  article.dataset.itemId = item.id;
  const meta = node('div', 'item-meta');
  meta.append(node('span', 'item-author', item.deviceName || '已配对设备'), node('span', 'item-time', readableDate(item.createdAt, true)));
  if (item.project) meta.append(node('span', 'project-tag', `# ${item.project}`));
  article.append(meta);
  const surface = node('div', 'item-surface');
  if (item.kind === 'file') renderFileContent(item, surface);
  else renderTextContent(item, surface);
  article.append(surface);
  return article;
}
function renderTextContent(item, surface) {
  if (item.kind === 'feedback') {
    const kicker = node('div', 'feedback-kicker'); kicker.append(node('span', '', '测试反馈'));
    if (item.buildId) kicker.append(buildReference(item.buildId));
    surface.append(kicker);
  }
  const text = node('div', `item-text${(item.text || '').length > 900 ? ' collapsed' : ''}`, item.text || '');
  surface.append(text);
  if (item.kind === 'feedback' && item.attachmentIds?.length) {
    const attachments = node('div', 'feedback-files');
    item.attachmentIds.forEach((id, index) => {
      const file = itemById(id);
      const link = textLink('', fileUrl(id), 'attachment-pill');
      link.append(node('span', '', file?.name || `附件 ${index + 1}`));
      link.title = '发起文件下载'; link.download = file?.name || '';
      attachments.append(link);
    });
    surface.append(attachments);
  }
  const actions = node('div', 'item-actions');
  actions.append(button('复制文字', 'text-button', () => void copyText(item.text || '')));
  const url = safeHttpUrl(item.text);
  if (url) {
    const link = textLink('打开链接 ↗', url); link.target = '_blank'; link.rel = 'noopener noreferrer'; actions.append(link);
  }
  if (text.classList.contains('collapsed')) {
    const expand = button('展开全文', 'text-button', () => { text.classList.toggle('collapsed'); expand.textContent = text.classList.contains('collapsed') ? '展开全文' : '收起'; });
    actions.append(expand);
  }
  if (item.kind === 'feedback') {
    const exportLink = textLink('导出反馈 Markdown', `/api/feedback/${encodeURIComponent(item.id)}/export`); exportLink.download = ''; actions.append(exportLink);
  }
  actions.append(button('详情', 'text-button', () => openItemDetail(item.id)));
  surface.append(actions);
}
function buildReference(id) {
  const build = itemById(id) || (state.currentBuild?.id === id ? state.currentBuild : null);
  return button(`关联 ${build?.name || '安装包'}`, 'build-reference', () => openItemDetail(id));
}
function fileType(item) {
  if (item.isApk) return { label: 'APK', type: 'apk' };
  if (/^image\//.test(item.mime || '')) return { label: '图片', type: 'image' };
  if (/^video\//.test(item.mime || '')) return { label: '视频', type: 'video' };
  if (/^audio\//.test(item.mime || '')) return { label: '音频', type: 'audio' };
  const ext = (item.name || '').split('.').pop().slice(0, 6).toUpperCase();
  if (['JSON', 'JS', 'TS', 'TXT', 'LOG', 'MD', 'HTML', 'CSS', 'XML'].includes(ext)) return { label: ext, type: 'code' };
  return { label: ext && ext !== item.name?.toUpperCase() ? ext : '文件', type: '' };
}
function renderFileContent(item, surface) {
  const row = node('div', 'file-surface');
  const type = fileType(item);
  if (IMAGE_MIMES.has(item.mime) && item.size <= 20 * 1024 * 1024) {
    const preview = button('', 'thumbnail-button', () => openItemDetail(item.id));
    preview.setAttribute('aria-label', `预览 ${item.name}`);
    const image = node('img', 'file-thumb'); image.alt = item.name; image.src = fileUrl(item.id, true); image.loading = 'lazy'; image.decoding = 'async';
    image.addEventListener('error', () => { preview.replaceChildren(node('span', 'file-icon image', '图片')); }, { once: true });
    preview.append(image); row.append(preview);
  } else row.append(node('span', `file-icon ${type.type}`, type.label));
  const summary = node('div', 'file-summary');
  summary.append(node('strong', 'file-name', item.name));
  const sub = node('div', 'file-subline'); sub.append(node('span', '', formatBytes(item.size)), node('span', 'dot'), node('span', '', '已存到电脑'));
  if (item.apk?.versionName) sub.append(node('span', 'dot'), node('span', '', `v${item.apk.versionName}`));
  summary.append(sub); row.append(summary);
  const actions = node('div', 'file-actions');
  const download = textLink('下载 ↓', fileUrl(item.id), 'button quiet small'); download.download = item.name; download.title = '由浏览器发起下载；不代表已安装或完成测试';
  const details = button('⋯', 'icon-button', () => openItemDetail(item.id)); details.setAttribute('aria-label', `查看 ${item.name} 详情`);
  actions.append(download, details); row.append(actions); surface.append(row);
  if (item.isApk) {
    const footer = node('div', 'apk-footer');
    const statusText = TEST_STATUSES.find(([value]) => value === item.testStatus)?.[1] || '尚未标记测试';
    const statusClass = item.testStatus === 'user-confirmed-passed' ? ' passed' : item.testStatus === 'issue-found' ? ' issue' : '';
    footer.append(node('span', `apk-status${statusClass}`, statusText));
    footer.append(button(state.currentBuild?.id === item.id ? '当前测试版本 ✓' : '选为当前测试版本', 'text-button', () => selectBuild(item)));
    surface.append(footer);
  } else if (item.buildId) {
    const footer = node('div', 'item-actions'); footer.append(buildReference(item.buildId)); surface.append(footer);
  }
}
function selectBuild(item) {
  if (!item?.isApk) return;
  state.currentBuild = item;
  storageSet(localStorage, deviceKey('current-build'), { id: item.id, name: item.name, project: item.project, apk: item.apk, isApk: true });
  $('projectInput').value = item.project || '';
  renderCurrentBuild(); renderFeed(); persistDraft();
  toast('已选中，接下来发送的内容会关联这个测试版本');
}
function renderCurrentBuild() {
  const build = state.currentBuild;
  $('currentBuild').hidden = !build;
  if (build) {
    $('currentBuildName').textContent = build.name || '已选安装包';
    const details = [build.apk?.versionName ? `v${build.apk.versionName}` : '', build.apk?.packageName || ''];
    $('currentBuildMeta').textContent = details.filter(Boolean).join(' · ') || '手动选择 · 不代表已安装或测试通过';
  }
  renderComposerMode();
}
function renderComposerMode() {
  const feedback = state.mode === 'feedback';
  $('textMode').classList.toggle('selected', !feedback); $('textMode').setAttribute('aria-pressed', String(!feedback));
  $('feedbackMode').classList.toggle('selected', feedback); $('feedbackMode').setAttribute('aria-pressed', String(feedback));
  $('pickAttachments').hidden = !feedback;
  $('composerText').placeholder = feedback ? '说说刚才发生了什么，或希望它怎样改进…' : '写点什么，或把文件拖到这里…';
  $('sendLabel').textContent = feedback ? '发反馈' : '发送';
  $('composerHint').textContent = feedback ? (state.currentBuild ? '会关联当前测试版本' : '可先在安装包中选择测试版本') : '原图传递 · 文件保存在电脑';
  renderSelectedAttachments();
}
function renderSelectedAttachments() {
  const container = $('selectedAttachments');
  container.hidden = state.mode !== 'feedback' || !state.attachments.size;
  const nodes = [];
  for (const id of state.attachments) {
    const item = itemById(id);
    const pill = node('span', 'attachment-pill'); pill.append(node('span', '', item?.name || '已选择的文件'));
    const remove = button('×', 'remove-attachment', () => { state.attachments.delete(id); renderSelectedAttachments(); persistDraft(); });
    remove.setAttribute('aria-label', `移除附件 ${item?.name || ''}`); pill.append(remove); nodes.push(pill);
  }
  container.replaceChildren(...nodes);
}
async function sendText(event) {
  event.preventDefault();
  if (state.sending) return;
  const text = $('composerText').value;
  if (!text.trim()) { setError('composerError', state.mode === 'feedback' ? '写一句现场描述，再发送反馈。' : '先写点文字，或选择要传的文件。'); $('composerText').focus(); return; }
  if (Array.from(text).length > 100000) { setError('composerError', '文字超过 100,000 字符。可以保存为文本文件后传递。'); return; }
  if (state.mode === 'feedback' && state.jobs.some((job) => job.attachToFeedback && !FINISHED_STATES.has(job.status))) {
    setError('composerError', '反馈附件还没传完。可以等待完成，或在队列中取消未完成的附件后再发送。'); return;
  }
  const project = $('projectInput').value.trim();
  state.sending = true; $('sendText').disabled = true; setError('composerError');
  try {
    const body = { text, project, ...(state.currentBuild ? { buildId: state.currentBuild.id } : {}) };
    const endpoint = state.mode === 'feedback' ? '/api/feedback' : '/api/items/text';
    if (state.mode === 'feedback') body.attachmentIds = [...state.attachments];
    const result = await api(endpoint, { method: 'POST', body });
    // Only clear the text that was sent; edits made while the request was in flight survive.
    if ($('composerText').value === text) $('composerText').value = '';
    if (endpoint === '/api/feedback') for (const id of body.attachmentIds) state.attachments.delete(id);
    mergeItem(result.item); renderSelectedAttachments(); persistDraft();
    toast(endpoint === '/api/feedback' ? '反馈已保存，可在电脑上导出' : '已发送');
  } catch (error) { setError('composerError', error.message); }
  finally { state.sending = false; $('sendText').disabled = false; }
}
function mergeItem(item) {
  if (!item?.id) return;
  const index = state.items.findIndex((entry) => entry.id === item.id);
  if (index >= 0) state.items[index] = item; else state.items.unshift(item);
  state.items.sort((a, b) => b.createdAt.localeCompare(a.createdAt) || b.id.localeCompare(a.id));
  state.feedLoaded = true; $('feed').setAttribute('aria-busy', 'false');
  renderFeed(); renderFilters(); renderProjectSuggestions();
  if (state.currentBuild?.id === item.id) { state.currentBuild = item; renderCurrentBuild(); }
}

async function openConnection() {
  if (!state.device) return;
  openDialog('connectionDialog');
  if (state.connectionBusy) return;
  state.connectionBusy = true;
  $('connectionLoading').hidden = false; $('connectionContent').hidden = true; setError('connectionError');
  try {
    const results = await Promise.allSettled([loadInfo(), loadDevices(), loadRequests()]);
    for (const result of results) if (result.status === 'rejected') throw result.reason;
    $('ownerPairing').hidden = state.device.role !== 'owner';
    if (state.device.role === 'owner' && (!state.pairing || Date.parse(state.pairing.expiresAt) <= Date.now())) await generatePairing();
    else if (state.pairing) renderPairing();
    renderDevices(); renderRequests(); renderConnectionFacts();
    $('connectionContent').hidden = false;
  } catch (error) { setError('connectionError', error.message); }
  finally { state.connectionBusy = false; $('connectionLoading').hidden = true; }
}
async function generatePairing() {
  if (state.device?.role !== 'owner') return;
  $('refreshPairing').disabled = true;
  try {
    state.pairing = await api('/api/pairing', { method: 'POST', body: {} });
    renderPairing(); setError('connectionError');
  } finally { $('refreshPairing').disabled = false; }
}
function renderPairing() {
  const pairing = state.pairing;
  if (!pairing) return;
  const previousOrigin = state.pairingUrl ? new URL(state.pairingUrl).origin : '';
  const addresses = [];
  for (const address of pairing.urls?.length ? pairing.urls : state.info?.urls || []) {
    try {
      const url = new URL(address);
      if (!['http:', 'https:'].includes(url.protocol)) continue;
      url.hash = new URLSearchParams({ pair: pairing.token }).toString();
      if (!addresses.some((entry) => entry.url === url.href)) addresses.push({ url: url.href, label: url.origin, preview: false });
    } catch { /* Ignore an invalid address, never render it as HTML. */ }
  }
  if (!addresses.some((entry) => new URL(entry.url).origin === location.origin)) {
    const url = new URL('/', location.origin); url.hash = new URLSearchParams({ pair: pairing.token }).toString();
    addresses.push({ url: url.href, label: `${url.origin}（当前页面 / 预览入口）`, preview: true });
  }
  const selected = addresses.find((entry) => new URL(entry.url).origin === previousOrigin) || addresses[0];
  $('pairAddress').replaceChildren(...addresses.map((entry) => {
    const option = node('option', '', entry.label); option.value = entry.url; return option;
  }));
  state.pairingUrl = selected?.url || '';
  $('pairAddress').value = state.pairingUrl;
  $('pairingCode').textContent = pairing.code;
  renderPairQr(); updatePairExpiry();
  clearInterval(state.pairingTimer); state.pairingTimer = setInterval(updatePairExpiry, 1000);
}
function renderPairQr() {
  $('addressDisplay').textContent = state.pairingUrl ? new URL(state.pairingUrl).origin : '暂无可用地址';
  $('copyPairLink').disabled = !state.pairingUrl;
  if (!state.pairingUrl) { $('pairQr').replaceChildren(node('span', '', '请使用连接码')); return; }
  try {
    const documentSvg = new DOMParser().parseFromString(qrSvg(state.pairingUrl), 'image/svg+xml');
    const svg = documentSvg.documentElement;
    if (svg.localName !== 'svg' || documentSvg.querySelector('parsererror')) throw new Error('Invalid QR');
    svg.setAttribute('role', 'img'); svg.setAttribute('aria-label', '扫码申请连接此电脑');
    $('pairQr').replaceChildren(document.importNode(svg, true));
  } catch { $('pairQr').replaceChildren(node('span', '', '二维码暂不可用\n请使用六位连接码')); }
}
function updatePairExpiry() {
  if (!state.pairing) return;
  const seconds = Math.max(0, Math.ceil((Date.parse(state.pairing.expiresAt) - Date.now()) / 1000));
  $('pairExpiry').textContent = seconds ? `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')} 后过期 · 每个码仅供一次申请` : '连接码已过期，请生成新码';
  if (!seconds) { clearInterval(state.pairingTimer); $('pairQr').style.opacity = '.35'; }
  else $('pairQr').style.opacity = '1';
}
function renderRequests() {
  $('requestCount').textContent = state.requests.length ? String(state.requests.length) : '';
  if (!state.requests.length) { $('pairRequests').replaceChildren(node('p', 'empty-inline', '新设备扫码申请后，会出现在这里。')); return; }
  $('pairRequests').replaceChildren(...state.requests.map((request) => {
    const row = node('div', 'request-row');
    row.append(node('span', 'device-avatar', '↔'));
    const content = node('div', 'device-row-content');
    content.append(node('strong', '', request.name), node('span', '', `${readableDate(request.createdAt)} 申请连接`)); row.append(content);
    const actions = node('div', 'request-actions');
    const approve = button('确认连接', 'button primary small', () => void busyAction(approve, async () => {
      await api(`/api/pair/requests/${encodeURIComponent(request.id)}/approve`, { method: 'POST', body: {} });
      toast('已同意连接，等待对方页面完成接入'); await loadRequests(); await loadDevices();
    }));
    const reject = button('拒绝', 'button quiet small', () => void busyAction(reject, async () => {
      await api(`/api/pair/requests/${encodeURIComponent(request.id)}/reject`, { method: 'POST', body: {} });
      await loadRequests(); toast('已拒绝这次连接申请');
    }));
    actions.append(approve, reject); row.append(actions); return row;
  }));
}
function renderDevices() {
  $('deviceList').replaceChildren(...state.devices.map((device) => {
    const row = node('div', 'device-row'); row.append(node('span', 'device-avatar', device.role === 'owner' ? '▣' : '▯'));
    const content = node('div', 'device-row-content');
    content.append(node('strong', '', device.name));
    content.append(node('span', '', `${device.role === 'owner' ? '管理设备 · ' : ''}最近访问 ${readableDate(device.lastSeenAt || device.createdAt)}`));
    row.append(content);
    if (device.id === state.device?.id) row.append(node('span', 'self-tag', '此浏览器'));
    else if (state.device?.role === 'owner' && device.role !== 'owner') {
      const revoke = button('撤销连接', 'text-button danger', async () => {
        if (!await confirmAction('撤销这台设备的连接？', `「${device.name}」将无法继续访问内容流。以后需要重新配对。`, '撤销连接')) return;
        await busyAction(revoke, async () => { await api(`/api/devices/${encodeURIComponent(device.id)}`, { method: 'DELETE' }); await loadDevices(); toast('已撤销设备连接'); });
      }); row.append(revoke);
    }
    return row;
  }));
  if (!state.devices.length) $('deviceList').append(node('p', 'empty-inline', '暂未读取到配对设备。'));
}
function addFact(list, label, value) { list.append(node('dt', '', label), node('dd', '', value === null || value === undefined || value === '' ? '未知' : value)); }
function renderConnectionFacts() {
  const info = state.info;
  if (!info) return;
  $('transportNotice').textContent = info.secure
    ? '当前连接使用 HTTPS。移动浏览器仍可能暂停后台页面；长文件传输时请尽量保持页面在前台。'
    : '当前是局域网 HTTP 连接，传输未加密。请在可信网络中使用；自动剪贴板和后台传输能力受浏览器限制。';
  const facts = $('connectionFacts'); facts.replaceChildren();
  addFact(facts, '运行电脑', info.hostName || '未提供');
  addFact(facts, '本地地址', (info.urls || []).join('\n') || '未发现可用局域网地址');
  addFact(facts, '应用版本', info.version);
  addFact(facts, '单文件上限', formatBytes(info.maxFileBytes));
  addFact(facts, 'APK 信息解析', info.apkAnalyzerAvailable ? '内置清单解析 + Android SDK 增强' : '内置清单解析（无需 SDK）；签名与安装状态未知');
  if (info.dataDir && state.device?.role === 'owner') addFact(facts, '数据位置', info.dataDir);
}
function openItemDetail(id) {
  const item = itemById(id);
  if (!item) { toast('这条记录尚未加载，可以加载更早的记录后再查看。'); return; }
  $('detailTitle').textContent = item.kind === 'file' ? item.name : item.kind === 'feedback' ? '测试反馈' : '这段文字';
  $('detailEyebrow').textContent = item.isApk ? '安装包 · 版本与测试记录' : item.kind === 'file' ? '文件详情' : '传递详情';
  const content = $('detailContent'); content.replaceChildren();
  if (item.kind === 'file') {
    if (IMAGE_MIMES.has(item.mime)) {
      const image = node('img', 'detail-preview'); image.src = fileUrl(item.id, true); image.alt = item.name; image.decoding = 'async';
      image.addEventListener('error', () => { image.replaceWith(node('p', 'detail-notice', '浏览器未能预览这张图片。原文件仍可下载。')); }, { once: true });
      content.append(image);
    } else if (['video/mp4', 'video/webm', 'video/ogg'].includes(item.mime)) {
      const video = node('video', 'detail-preview video'); video.src = fileUrl(item.id, true); video.controls = true; video.preload = 'metadata'; video.playsInline = true;
      content.append(video);
    } else if (['audio/mpeg', 'audio/mp4', 'audio/ogg', 'audio/wav', 'audio/webm', 'audio/flac'].includes(item.mime)) {
      const audio = node('audio', 'detail-preview audio'); audio.src = fileUrl(item.id, true); audio.controls = true; audio.preload = 'metadata'; content.append(audio);
    }
    const facts = node('dl', 'facts');
    addFact(facts, '文件大小', formatBytes(item.size)); addFact(facts, '发送设备', item.deviceName); addFact(facts, '存到电脑', readableDate(item.createdAt));
    addFact(facts, '文件 SHA-256', item.sha256 || '未提供');
    if (item.isApk) {
      const apk = item.apk || {};
      addFact(facts, '应用名称', apk.label || '未解析'); addFact(facts, '包名', apk.packageName || '未解析');
      addFact(facts, '版本名', apk.versionName || '未解析'); addFact(facts, '版本号', apk.versionCode ?? '未解析');
      addFact(facts, '最低 Android API', apk.minSdk ?? '未解析'); addFact(facts, '目标 Android API', apk.targetSdk ?? '未解析');
      addFact(facts, '签名 SHA-256', apk.signatureSha256 || '未知；文件哈希不代表签名指纹');
      content.append(facts);
      content.append(node('p', 'detail-notice', apk.note || '文件按 .apk 扩展名识别。安装是否可行由 Android 系统决定；网页不会自动安装或清除应用数据。'));
    } else content.append(facts);
    const actions = node('div', 'detail-actions');
    const download = textLink('下载文件 ↓', fileUrl(item.id), 'button primary'); download.download = item.name;
    download.addEventListener('click', () => toast('已发起下载，请在浏览器的下载列表查看。'));
    actions.append(download, button('复制 SHA-256', 'button quiet', () => void copyText(item.sha256 || '')));
    if (item.isApk) actions.append(button('选为当前测试版本', 'button subtle', () => { selectBuild(item); closeDialog($('detailDialog')); }));
    if (state.device?.role === 'owner') {
      const locationButton = button('电脑保存位置', 'button quiet', () => void busyAction(locationButton, async () => {
        const result = await api(`/api/files/${encodeURIComponent(item.id)}/location`);
        let location = content.querySelector('.location-result');
        if (!location) { location = node('div', 'location-result'); content.append(location); }
        location.replaceChildren(node('p', '', result.path), button('复制路径', 'text-button', () => void copyText(result.path)));
      })); actions.append(locationButton);
    }
    content.append(actions);
    content.append(node('p', 'small-text muted', '下载按钮只发起浏览器下载，不表示文件已保存、应用已安装或测试已通过。'));
    content.lastChild.style.marginTop = '12px';
  } else {
    content.append(node('div', 'detail-text', item.text || ''));
    const actions = node('div', 'detail-actions'); actions.append(button('复制文字', 'button quiet', () => void copyText(item.text || '')));
    if (item.kind === 'feedback') {
      const download = textLink('导出反馈 Markdown', `/api/feedback/${encodeURIComponent(item.id)}/export`, 'button primary'); download.download = ''; actions.append(download);
    }
    content.append(actions);
    if (item.buildId) { const reference = node('div', 'detail-actions'); reference.append(buildReference(item.buildId)); content.append(reference); }
  }
  const form = node('form', 'detail-section'); form.append(node('h3', '', item.isApk ? '由你记录测试状态' : '给内容补一点说明'));
  const projectLabel = node('label', 'field', '项目标签（选填）'); const project = node('input'); project.value = item.project || ''; project.maxLength = 80; project.placeholder = '例如：养老笔记'; projectLabel.append(project); form.append(projectLabel);
  const noteLabel = node('label', 'field', '备注（选填）'); const note = node('textarea'); note.value = item.note || ''; note.maxLength = 5000; note.rows = 2; note.placeholder = '版本变化、文件用途，或需要记住的事'; noteLabel.append(note); form.append(noteLabel);
  let statusSelect;
  if (item.isApk) {
    const label = node('label', 'field', '你确认的实际状态'); statusSelect = node('select');
    for (const [value, text] of TEST_STATUSES) { const option = node('option', '', text); option.value = value; statusSelect.append(option); }
    statusSelect.value = item.testStatus || 'not-tested'; label.append(statusSelect); form.append(label);
    form.append(node('p', 'detail-notice', '这是你的手动记录。请选择已经实际确认的结果，下载完成不会自动修改这里。'));
  }
  const save = node('button', 'button primary small', '保存记录'); save.type = 'submit'; form.append(save);
  const error = node('p', 'inline-error'); error.hidden = true; form.append(error);
  form.addEventListener('submit', async (event) => {
    event.preventDefault(); save.disabled = true; error.hidden = true;
    try {
      const body = { project: project.value.trim(), note: note.value, ...(statusSelect ? { testStatus: statusSelect.value } : {}) };
      const result = await api(`/api/items/${encodeURIComponent(item.id)}`, { method: 'PATCH', body });
      mergeItem(result.item); toast('记录已保存'); closeDialog($('detailDialog'));
    } catch (failure) { error.textContent = failure.message; error.hidden = false; }
    finally { save.disabled = false; }
  });
  content.append(form); openDialog('detailDialog');
}
function openAttachments() {
  state.attachmentDraft = new Set(state.attachments);
  const candidates = state.items.filter((item) => item.kind === 'file').sort((a, b) => {
    const aBuild = state.currentBuild && a.buildId === state.currentBuild.id ? 1 : 0;
    const bBuild = state.currentBuild && b.buildId === state.currentBuild.id ? 1 : 0;
    return bBuild - aBuild || b.createdAt.localeCompare(a.createdAt);
  }).slice(0, 60);
  const list = $('attachmentList'); list.replaceChildren();
  if (!candidates.length) list.append(node('p', 'empty-inline', '还没有已上传的文件。可以先通过发送区上传截图或录屏。'));
  for (const item of candidates) {
    const label = node('label', 'attachment-option'); const checkbox = node('input'); checkbox.type = 'checkbox'; checkbox.value = item.id; checkbox.checked = state.attachmentDraft.has(item.id);
    checkbox.addEventListener('change', () => { if (checkbox.checked) state.attachmentDraft.add(item.id); else state.attachmentDraft.delete(item.id); updateAttachmentCount(); });
    const details = node('div'); details.append(node('strong', '', item.name), node('span', '', `${formatBytes(item.size)} · ${readableDate(item.createdAt)}${item.buildId === state.currentBuild?.id ? ' · 当前测试' : ''}`));
    label.append(checkbox, details); list.append(label);
  }
  updateAttachmentCount(); openDialog('attachmentDialog');
}
function updateAttachmentCount() { $('confirmAttachments').textContent = state.attachmentDraft.size ? `选好了（${state.attachmentDraft.size}）` : '选好了'; }

function localId() {
  const random = globalThis.crypto?.getRandomValues ? [...crypto.getRandomValues(new Uint32Array(2))].map((value) => value.toString(36)).join('') : Math.random().toString(36).slice(2);
  return `${Date.now().toString(36)}-${random}`;
}
function enqueueFiles(files) {
  if (!state.device) { toast('请先连接设备，再发送文件。', true); return; }
  const maxBytes = state.info?.maxFileBytes || 2 * 1024 ** 3;
  let added = 0;
  for (const file of Array.from(files)) {
    if (!(file instanceof File)) continue;
    if (file.size > maxBytes) { toast(`「${file.name}」超过单文件上限 ${formatBytes(maxBytes)}。`, true); continue; }
    state.jobs.push({
      localId: localId(), file, name: file.name, size: file.size, mime: file.type || 'application/octet-stream',
      status: 'queued', offset: 0, hashProgress: 0, sha256: '', uploadId: '', fileVerified: false,
      project: $('projectInput').value.trim(), buildId: state.currentBuild?.id || '',
      attachToFeedback: state.mode === 'feedback', createdAt: new Date().toISOString(),
      stopped: false, cancelRequested: false, error: '',
    });
    added += 1;
  }
  if (!added) return;
  persistQueue(); renderQueue(); pumpQueue();
  toast(`${added} 个文件已加入队列`);
}
function persistQueue() {
  if (!state.device) return;
  const saved = state.jobs.filter((job) => !FINISHED_STATES.has(job.status)).map((job) => ({
    localId: job.localId, uploadId: job.uploadId, name: job.name, size: job.size, mime: job.mime,
    sha256: job.sha256, offset: job.offset, project: job.project, buildId: job.buildId,
    attachToFeedback: job.attachToFeedback, createdAt: job.createdAt,
    cancelRequested: job.cancelRequested, creationAttempted: job.creationAttempted,
  }));
  storageSet(localStorage, deviceKey('queue'), saved);
}
async function restoreQueue() {
  const saved = storageGet(localStorage, deviceKey('queue'), []);
  const stored = Array.isArray(saved) ? saved.filter((entry) => entry && typeof entry.name === 'string' && Number.isSafeInteger(entry.size) && entry.size >= 0) : [];
  let uploads = [];
  let failed = null;
  try { const result = await api('/api/uploads'); uploads = result.uploads || []; }
  catch (error) { failed = error; }
  const pending = uploads.filter((upload) => upload.status !== 'complete');
  const used = new Set();
  const restored = [];
  for (const entry of stored) {
    let remote = uploads.find((upload) => upload.id === entry.uploadId);
    if (remote?.status === 'complete') { used.add(remote.id); continue; }
    if (!remote && !entry.uploadId && entry.sha256) remote = pending.find((upload) => !used.has(upload.id)
      && upload.size === entry.size && upload.sha256 === entry.sha256
      && (upload.project || '') === (entry.project || '') && (upload.buildId || '') === (entry.buildId || ''));
    if (entry.uploadId && !remote && !failed) continue;
    if (remote) used.add(remote.id);
    restored.push({
      ...entry, ...(remote || {}), localId: entry.localId || localId(), uploadId: remote?.id || entry.uploadId || '',
      file: null, fileVerified: false, status: entry.cancelRequested ? 'cancel-pending' : 'needs-file',
      offset: remote?.offset ?? entry.offset ?? 0, hashProgress: 0, stopped: false, error: '',
    });
  }
  for (const upload of pending) if (!used.has(upload.id)) {
    restored.push({ ...upload, localId: localId(), uploadId: upload.id, file: null, fileVerified: false, status: 'needs-file', hashProgress: 0, stopped: false, error: '' });
  }
  state.jobs = restored; persistQueue(); renderQueue();
  if (failed) throw failed;
}
function scheduleQueueRender() {
  if (state.renderQueueTimer) return;
  state.renderQueueTimer = setTimeout(() => { state.renderQueueTimer = null; renderQueue(); }, 100);
}
function queueStatus(job) {
  switch (job.status) {
    case 'queued': return '等待传输';
    case 'hashing': return `正在核对完整文件指纹 · ${Math.round((job.hashProgress || 0) * 100)}%`;
    case 'uploading': return `${formatBytes(job.offset)} / ${formatBytes(job.size)} · ${job.size ? Math.floor(job.offset / job.size * 100) : 100}%`;
    case 'completing': return '电脑正在校验完整文件，请稍等…';
    case 'complete': return '完整文件已存到电脑 · SHA-256 已校验';
    case 'paused': return `已暂停 · 电脑已接收 ${formatBytes(job.offset)}`;
    case 'needs-file': return job.error || (job.uploadId ? `已接收 ${formatBytes(job.offset)} · 重选原文件并校验后续传` : '重选原文件，继续这次传递');
    case 'canceling': return '已停止发送，正在清理电脑上的临时文件…';
    case 'cancel-pending': return '已停止发送，电脑临时文件尚未清理，请重试清理';
    case 'canceled': return '已取消，临时文件已清理';
    case 'error': return job.error || '传输中断，可以重试';
    default: return '等待处理';
  }
}
function renderQueue() {
  $('queuePanel').hidden = state.jobs.length === 0;
  const pending = state.jobs.filter((job) => !FINISHED_STATES.has(job.status)).length;
  $('queueCount').textContent = pending ? `${pending} 项待完成` : '全部已结束';
  $('clearFinished').hidden = !state.jobs.some((job) => FINISHED_STATES.has(job.status));
  const existingIds = new Set(state.jobs.map((job) => job.localId));
  for (const [id, entry] of state.queueNodes) if (!existingIds.has(id)) { entry.row.remove(); state.queueNodes.delete(id); }
  for (const job of state.jobs) {
    let elements = state.queueNodes.get(job.localId);
    if (!elements) {
      const row = node('div', 'queue-row'); row.dataset.queueId = job.localId;
      const summary = node('div'); const name = node('div', 'queue-name', job.name); name.title = job.name;
      const status = node('div', 'queue-status'); summary.append(name, status);
      const controls = node('div', 'queue-controls'); const progress = node('div', 'queue-progress'); const fill = node('span'); progress.append(fill);
      progress.setAttribute('role', 'progressbar'); progress.setAttribute('aria-label', `${job.name} 传输进度`); progress.setAttribute('aria-valuemin', '0'); progress.setAttribute('aria-valuemax', '100');
      row.append(summary, controls, progress); $('queueList').append(row);
      elements = { row, name, status, controls, progress, fill, controlKey: '' }; state.queueNodes.set(job.localId, elements);
    }
    elements.row.dataset.state = job.status;
    elements.status.textContent = queueStatus(job); elements.status.classList.toggle('error', ['error', 'cancel-pending'].includes(job.status));
    const percent = job.status === 'complete' ? 100 : job.status === 'hashing' ? (job.hashProgress || 0) * 100 : job.size ? job.offset / job.size * 100 : 0;
    elements.fill.style.width = `${Math.min(100, Math.max(0, percent))}%`;
    elements.progress.setAttribute('aria-valuenow', String(Math.round(percent)));
    elements.progress.hidden = ['canceled', 'cancel-pending', 'canceling'].includes(job.status);
    const controlKey = `${job.status}|${Boolean(job.file)}`;
    if (elements.controlKey !== controlKey) {
      elements.controlKey = controlKey; const controls = [];
      if (['needs-file', 'paused', 'error'].includes(job.status)) controls.push(button(job.file ? '重试' : job.uploadId ? '重选原文件' : '重选文件', 'text-button', () => retryJob(job)));
      if (['uploading', 'hashing'].includes(job.status)) controls.push(button('暂停', 'text-button', () => pauseJob(job)));
      if (['queued', 'needs-file', 'paused', 'error', 'hashing', 'uploading', 'completing'].includes(job.status)) controls.push(button('取消', 'text-button danger', () => void cancelJob(job)));
      if (job.status === 'cancel-pending') controls.push(button('重试清理', 'text-button', () => void cancelJob(job)));
      if (job.status === 'complete' && job.itemId) controls.push(button('查看', 'text-button', () => openItemDetail(job.itemId)));
      elements.controls.replaceChildren(...controls);
    }
  }
}
function pumpQueue() {
  if (!state.device || state.activeJobs >= 1) return;
  const job = state.jobs.find((entry) => entry.status === 'queued' && entry.file);
  if (!job) return;
  state.activeJobs += 1;
  job.running = runJob(job).finally(() => { state.activeJobs = Math.max(0, state.activeJobs - 1); job.running = null; queueMicrotask(pumpQueue); });
}
function hashFile(job) {
  return new Promise((resolve, reject) => {
    let settled = false;
    const worker = new Worker(new URL('./hash-worker.mjs', import.meta.url), { type: 'module' }); job.worker = worker;
    const finish = (value, error) => {
      if (settled) return; settled = true; worker.terminate(); job.worker = null; job.cancelHash = null;
      if (error) reject(error); else resolve(value);
    };
    job.cancelHash = () => finish(null, new DOMException('Hash canceled', 'AbortError'));
    worker.addEventListener('message', (event) => {
      const result = event.data || {};
      if (result.id !== job.localId) return;
      if (result.error) { finish(null, new Error(`文件指纹计算失败：${result.error}`)); return; }
      if (result.sha256) { finish(result.sha256); return; }
      if (Number.isFinite(result.progress)) { job.hashProgress = Math.max(0, Math.min(1, result.progress)); scheduleQueueRender(); }
    });
    worker.addEventListener('error', () => finish(null, new Error('文件指纹计算未能启动。请保持页面打开并重试。')));
    worker.postMessage({ id: job.localId, file: job.file });
  });
}
function assertRunning(job) { if (job.stopped || job.cancelRequested || !state.device) throw new DOMException('Transfer stopped', 'AbortError'); }
async function refreshUpload(job, signal) {
  const result = await api(`/api/uploads/${encodeURIComponent(job.uploadId)}`, { signal });
  const upload = result.upload;
  if (!upload) throw new Error('电脑未返回上传状态，请重试。');
  if (upload.sha256 !== job.sha256 || upload.size !== job.size) throw new Error('电脑记录的文件指纹与原文件不一致，已停止传输。');
  job.offset = upload.offset; job.chunkSize = upload.chunkSize;
  if (upload.status === 'complete') {
    job.itemId = upload.itemId; job.status = 'complete'; job.offset = job.size;
    await loadItems(); return upload;
  }
  if (upload.status === 'failed') throw new Error('这次上传已失效。请取消并清理后，重新添加原文件。');
  return upload;
}
async function recoverCreatedUpload(job) {
  if (!job.creationAttempted || !job.sha256 || job.uploadId) return;
  const result = await api('/api/uploads');
  const usedIds = new Set(state.jobs.filter((entry) => entry !== job).map((entry) => entry.uploadId).filter(Boolean));
  const matches = (result.uploads || []).filter((upload) => !usedIds.has(upload.id) && upload.status !== 'complete'
    && upload.size === job.size && upload.sha256 === job.sha256
    && (upload.project || '') === (job.project || '') && (upload.buildId || '') === (job.buildId || ''));
  if (matches.length === 1) { job.uploadId = matches[0].id; job.offset = matches[0].offset; job.chunkSize = matches[0].chunkSize; }
  else if (matches.length > 1) throw new Error('找到多条相同文件的未完成传输。请刷新页面，在恢复队列中选择要继续的那一条。');
}
async function runJob(job) {
  job.stopped = false; job.cancelRequested = false; job.error = '';
  const controller = new AbortController(); job.controller = controller;
  try {
    assertRunning(job);
    if (!job.fileVerified) {
      job.status = 'hashing'; job.hashProgress = 0; renderQueue();
      const actualSha = (await hashFile(job)).toLowerCase();
      assertRunning(job);
      if (job.sha256 && job.sha256.toLowerCase() !== actualSha) {
        job.file = null; job.status = 'needs-file'; job.error = '所选文件内容不一致，请重选原文件。';
        toast(`「${job.name}」内容与原文件不同，未发送任何新数据。`, true); return;
      }
      job.sha256 = actualSha; job.fileVerified = true; job.hashProgress = 1; persistQueue();
    }
    assertRunning(job);
    await recoverCreatedUpload(job);
    if (job.uploadId) {
      const upload = await refreshUpload(job, controller.signal);
      if (upload.status === 'complete') { finishUpload(job); return; }
    } else {
      job.creationAttempted = true; persistQueue();
      const result = await api('/api/uploads', {
        method: 'POST', signal: controller.signal,
        body: { name: job.name, size: job.size, mime: job.mime, sha256: job.sha256, project: job.project || '', ...(job.buildId ? { buildId: job.buildId } : {}) },
      });
      job.uploadId = result.upload.id; job.offset = result.upload.offset; job.chunkSize = result.upload.chunkSize; persistQueue();
    }
    assertRunning(job);
    job.status = 'uploading'; renderQueue();
    let conflictRetries = 0;
    while (job.offset < job.size) {
      assertRunning(job);
      const chunkSize = Math.min(Number(job.chunkSize) || 2 * 1024 ** 2, 2 * 1024 ** 2);
      const chunk = job.file.slice(job.offset, Math.min(job.size, job.offset + chunkSize));
      try {
        const result = await api(`/api/uploads/${encodeURIComponent(job.uploadId)}`, {
          method: 'PATCH', body: chunk, signal: controller.signal,
          headers: { 'Content-Type': 'application/octet-stream', 'Upload-Offset': String(job.offset) },
        });
        const nextOffset = result.upload?.offset;
        if (!Number.isSafeInteger(nextOffset) || nextOffset <= job.offset || nextOffset > job.size) throw new Error('电脑返回了异常进度，已停止传输。请重试以重新核对。');
        job.offset = nextOffset; conflictRetries = 0; persistQueue(); scheduleQueueRender();
      } catch (error) {
        if (error.status === 409 && conflictRetries < 2 && !job.stopped && !job.cancelRequested) {
          conflictRetries += 1;
          const remote = await refreshUpload(job, controller.signal);
          if (remote.status === 'complete') { finishUpload(job); return; }
          persistQueue(); continue;
        }
        throw error;
      }
    }
    assertRunning(job);
    job.status = 'completing'; renderQueue();
    const result = await api(`/api/uploads/${encodeURIComponent(job.uploadId)}/complete`, { method: 'POST', body: {}, signal: controller.signal });
    job.itemId = result.item.id; mergeItem(result.item); finishUpload(job);
    void loadInfo().catch(() => {});
  } catch (error) {
    if (job.stopped || job.cancelRequested || error.name === 'AbortError') {
      if (!['canceling', 'cancel-pending', 'canceled', 'complete'].includes(job.status)) job.status = 'paused';
    } else {
      job.status = 'error'; job.error = error.message || '传输中断，可以重试。';
      toast(`「${job.name}」未完成：${job.error}`, true);
    }
  } finally { job.controller = null; persistQueue(); renderQueue(); }
}
function finishUpload(job) {
  job.status = 'complete'; job.offset = job.size; job.file = null; job.fileVerified = false;
  if (job.attachToFeedback && job.itemId) { state.attachments.add(job.itemId); renderSelectedAttachments(); persistDraft(); }
  persistQueue(); renderQueue();
  toast(`「${job.name}」已完整存到电脑`);
}
function pauseJob(job) {
  job.stopped = true; job.controller?.abort(); job.cancelHash?.();
  if (!FINISHED_STATES.has(job.status)) job.status = 'paused';
  persistQueue(); renderQueue();
}
function retryJob(job) {
  if (job.running) { toast('正在结束上一次传输，请稍后重试。'); return; }
  if (!job.file) {
    state.resumeTarget = job.localId; $('resumeInput').value = ''; $('resumeInput').click(); return;
  }
  job.stopped = false; job.cancelRequested = false; job.error = ''; job.status = 'queued';
  persistQueue(); renderQueue(); pumpQueue();
}
async function resumeSelectedFile(event) {
  const file = event.target.files?.[0];
  const job = state.jobs.find((entry) => entry.localId === state.resumeTarget); state.resumeTarget = null;
  if (!file || !job) return;
  if (file.size !== job.size) { toast('文件大小不同，请选择最初传输的那个文件。', true); return; }
  if (!job.sha256 && file.name !== job.name) { toast('这次传递还未建立文件指纹，请选择最初添加的文件。', true); return; }
  job.file = file; job.fileVerified = false; job.hashProgress = 0;
  retryJob(job);
}
async function cancelJob(job) {
  if (job.status === 'complete' || job.status === 'canceled' || job.status === 'canceling') return;
  job.cancelRequested = true; job.stopped = true; job.status = 'canceling';
  job.controller?.abort(); job.cancelHash?.(); renderQueue(); persistQueue();
  try {
    if (job.running) await job.running;
    if (!job.uploadId && job.creationAttempted) await recoverCreatedUpload(job);
    if (job.uploadId) {
      try { await api(`/api/uploads/${encodeURIComponent(job.uploadId)}`, { method: 'DELETE' }); }
      catch (error) {
        if (error.status !== 404) {
          if (error.status === 409) {
            const result = await api(`/api/uploads/${encodeURIComponent(job.uploadId)}`);
            if (result.upload?.status === 'complete') {
              job.itemId = result.upload.itemId; await loadItems(); finishUpload(job);
              toast('文件已经完整存入电脑，不能再通过取消上传移除。'); return;
            }
          }
          throw error;
        }
      }
    }
    job.status = 'canceled'; job.file = null; job.uploadId = ''; job.cancelRequested = false;
    void loadInfo().catch(() => {});
  } catch (error) { job.status = 'cancel-pending'; job.error = error.message; toast('发送已停止；连接恢复后请重试清理临时文件。', true); }
  finally { persistQueue(); renderQueue(); }
}

function bindEvents() {
  $('joinForm').addEventListener('submit', submitJoin);
  $('pairCode').addEventListener('input', () => { $('pairCode').value = $('pairCode').value.replace(/\D/g, '').slice(0, 6); });
  $('cancelJoin').addEventListener('click', () => { clearTimeout(state.claimTimer); state.pairingRequest = null; storageSet(sessionStorage, 'lan-relay.pending', null); joinToken = ''; showJoin(); });
  for (const id of ['connectButton', 'mobileConnect', 'connectionDetailsButton']) $(id).addEventListener('click', () => void openConnection());
  $('refreshPairing').addEventListener('click', () => void generatePairing().catch((error) => setError('connectionError', error.message)));
  $('pairAddress').addEventListener('change', () => { state.pairingUrl = $('pairAddress').value; renderPairQr(); });
  $('copyPairLink').addEventListener('click', () => void copyText(state.pairingUrl));
  $('logoutButton').addEventListener('click', async () => {
    if (!await confirmAction('断开此浏览器的登录？', '下次使用需要通过管理链接或重新配对进入。正在进行的传输会暂停。', '断开登录')) return;
    await busyAction($('logoutButton'), async () => { await api('/api/session/logout', { method: 'POST', body: {} }); disconnectUI(); });
  });
  for (const control of document.querySelectorAll('.close-dialog')) control.addEventListener('click', () => closeDialog(control.closest('dialog')));
  for (const dialog of document.querySelectorAll('dialog')) dialog.addEventListener('click', (event) => {
    if (event.target !== dialog) return;
    const bounds = dialog.getBoundingClientRect();
    if (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom) closeDialog(dialog);
  });
  $('detailDialog').addEventListener('close', () => { for (const media of $('detailContent').querySelectorAll('video,audio')) media.pause(); });
  $('connectionDialog').addEventListener('close', () => clearInterval(state.pairingTimer));
  $('confirmCancel').addEventListener('click', () => closeDialog($('confirmDialog')));
  $('confirmAccept').addEventListener('click', () => { const resolve = confirmResolve; confirmResolve = null; closeDialog($('confirmDialog')); resolve?.(true); });
  $('confirmDialog').addEventListener('close', () => { const resolve = confirmResolve; confirmResolve = null; resolve?.(false); });
  $('searchInput').addEventListener('input', () => { state.query = $('searchInput').value; renderFeed(); });
  $('retryFeed').addEventListener('click', () => void busyAction($('retryFeed'), () => loadItems(!state.feedLoaded)));
  $('loadMore').addEventListener('click', () => void loadMore());
  $('composer').addEventListener('submit', sendText);
  for (const id of ['composerText', 'projectInput']) $(id).addEventListener('input', () => { setError('composerError'); persistDraft(); });
  $('composerText').addEventListener('keydown', (event) => { if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) { event.preventDefault(); $('composer').requestSubmit(); } });
  $('textMode').addEventListener('click', () => { state.mode = 'text'; renderComposerMode(); persistDraft(); });
  $('feedbackMode').addEventListener('click', () => { state.mode = 'feedback'; renderComposerMode(); persistDraft(); });
  $('writeFeedback').addEventListener('click', () => { state.mode = 'feedback'; renderComposerMode(); persistDraft(); $('composerText').focus(); });
  $('clearBuild').addEventListener('click', () => { state.currentBuild = null; storageSet(localStorage, deviceKey('current-build'), null); renderCurrentBuild(); renderFeed(); toast('已取消当前测试版本的关联'); });
  $('pickAttachments').addEventListener('click', openAttachments);
  $('confirmAttachments').addEventListener('click', () => { state.attachments = new Set(state.attachmentDraft); renderSelectedAttachments(); persistDraft(); closeDialog($('attachmentDialog')); });
  $('pickFiles').addEventListener('click', () => $('fileInput').click());
  $('fileInput').addEventListener('change', (event) => { enqueueFiles(event.target.files); event.target.value = ''; });
  $('resumeInput').addEventListener('change', (event) => void resumeSelectedFile(event));
  $('clearFinished').addEventListener('click', () => { state.jobs = state.jobs.filter((job) => !FINISHED_STATES.has(job.status)); persistQueue(); renderQueue(); });
  document.addEventListener('paste', (event) => {
    if (!state.device || document.querySelector('dialog[open]')) return;
    const files = event.clipboardData?.files;
    if (files?.length) { event.preventDefault(); enqueueFiles(files); }
  });
  document.addEventListener('dragenter', (event) => {
    if (!state.device || !Array.from(event.dataTransfer?.types || []).includes('Files')) return;
    event.preventDefault(); dragDepth += 1; $('dropOverlay').hidden = false;
  });
  document.addEventListener('dragover', (event) => {
    if (!state.device || !Array.from(event.dataTransfer?.types || []).includes('Files')) return;
    event.preventDefault(); event.dataTransfer.dropEffect = 'copy';
  });
  document.addEventListener('dragleave', (event) => {
    if (!state.device) return;
    event.preventDefault(); dragDepth = Math.max(0, dragDepth - 1); if (!dragDepth) $('dropOverlay').hidden = true;
  });
  document.addEventListener('drop', (event) => {
    if (!state.device || !Array.from(event.dataTransfer?.types || []).includes('Files')) return;
    event.preventDefault(); dragDepth = 0; $('dropOverlay').hidden = true;
    const entries = Array.from(event.dataTransfer?.items || []).map((item) => item.webkitGetAsEntry?.());
    if (entries.some((entry) => entry?.isDirectory)) toast('文件夹请先打包为 ZIP 后传递；普通文件仍会加入队列。');
    enqueueFiles(event.dataTransfer?.files || []);
  });
  document.addEventListener('keydown', (event) => {
    if (event.key === '/' && state.device && !['INPUT', 'TEXTAREA', 'SELECT'].includes(document.activeElement?.tagName) && !document.querySelector('dialog[open]')) {
      event.preventDefault(); $('searchInput').focus();
    }
  });
  document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'visible' && state.device) { void verifySession(); scheduleRefresh(); } });
  window.addEventListener('offline', () => { if (state.device) setConnectionStatus('offline'); });
  window.addEventListener('online', () => { if (state.device) { setConnectionStatus('connecting'); scheduleRefresh(); } });
  window.addEventListener('pagehide', () => { persistDraft(); persistQueue(); });
  window.addEventListener('beforeunload', (event) => {
    if (state.jobs.some((job) => ACTIVE_STATES.has(job.status))) { event.preventDefault(); event.returnValue = ''; }
  });
}

bindEvents();
void initialize();
