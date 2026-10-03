import {showDetailState,showOpenRetry} from './detail-notices'
import {viewKey} from '../../domain/runtime/messages.js'
import {retryDelay} from '../../domain/runtime/read-failures.js'

// Selection cancellation, cache hydration and native fork/continuation following
// belong to the read service, never to sidebar or message view callbacks.
export function createSessionOpen(environment, state, messageCache, reader, sync, index, terminal, composer, rendering, presentation) {
  const {capabilities, MOBILE, store} = environment;
  const {selection, catalog, unread} = state;
  const {cache} = messageCache;
  const cachePut = (...args) => messageCache.cachePut(...args);
  const cacheGet = (...args) => messageCache.cacheGet(...args);
  const fetchMessages = (...args) => reader.fetchMessages(...args);
  const {closeWatch, syncSession, watchSession, reportReadFailure, migrationReadFailures} = sync;
  const {hiddenForkParent, forkLeaf, forkLeafUid} = index;
  const {browserAuditEvent, showMobileDetail, syncSessionStopNotice, clearUnread,
    revealSessionInSidebar, paintSidebarSelection, renderSession, ensureConsolePlaceholder,
    auditDetailRendered, progress, progressDone, esc} = presentation;
  const requests = {inflight: null};
  const $ = selector => document.querySelector(selector);
function followContinuedSession(uid) {
  const seen = new Set();
  let cur = uid;
  while (cur && !seen.has(cur)) {
    seen.add(cur);
    const next = catalog.sessions.find(s => s.uid === cur)?.continued_in;
    if (!next || next === cur || !catalog.sessions.some(s => s.uid === next)) return cur;
    cur = next;
  }
  return cur;
}

// 首次打开时的瞬时失败（503、断网）：正文保留“读取失败”，按退避自动重试
// 三次（1.5/3/6 s），期间可手动重试；仍失败则停在可点击重试的提示上。
const openRetries = new Map();
function scheduleOpenRetry(uid, agent, ac) {
  if (capabilities.config.backend !== 'rust') return;
  const key = viewKey(uid, agent);
  const attempt = (openRetries.get(key) || 0) + 1;
  const retry = () => {
    if (selection.sel !== uid || selection.agent !== agent || requests.inflight !== ac) return;
    openRetries.set(key, attempt);
    openSession(uid, agent);
  };
  showOpenRetry(retry,attempt);
  if (attempt <= 3) {
    const timer = setTimeout(retry, retryDelay(attempt - 1));
    ac.signal.addEventListener('abort', () => clearTimeout(timer), {once: true});
  } else openRetries.delete(key);
}

// 当前会话被 Codex 回退成隐藏的父会话后，对话页和控制台一起跟到最深的分支：
// 同一个 pty 仍在写，只是原生叶子换了。用户主动打开的父会话（父会话链、
// 显示父会话）不在此列。
async function followSelectedFork() {
  const current = catalog.sessions.find(s => s.uid === selection.sel);
  if (!current || selection.agent || !hiddenForkParent(current)) return false;
  // 手机停在列表页时不把人拽进详情；父会话已从列表消失，点开分支即可。
  if (MOBILE.matches && store.get('mobilePage', 'list') !== 'detail') return false;
  const leaf = forkLeaf(current);
  if (!leaf || leaf.uid === selection.sel) return false;
  browserAuditEvent('session.fork_followed', {from_uid: selection.sel}, null, {uid: leaf.uid});
  if (typeof composer.migrateComposerDraft === 'function') composer.migrateComposerDraft(selection.sel, leaf.uid);
  if (typeof terminal.state !== 'undefined' && terminal.state.uid === selection.sel) terminal.state.uid = leaf.uid;
  await openSession(leaf.uid, null, {exact: true});
  return true;
}

/** One shareable route for sidebar navigation and external research links. */
function sessionUrl(uid, agent = null) {
  const row = catalog.sessions.find(s => s.uid === uid);
  if (!row?.sid) return null;
  const url = new URL(location.href);
  url.searchParams.set('sid', `${row.source}:${row.sid}${agent ? '/agent:' + agent : ''}`);
  if (row.node_id) url.searchParams.set('node', row.node_id);
  else url.searchParams.delete('node');
  return url;
}
function updateSessionUrl(uid, agent, mode) {
  const url = sessionUrl(uid, agent);
  if (!url || mode === 'none' || url.href === location.href) return;
  const method = mode === 'replace' || !new URL(location.href).searchParams.has('sid') ? 'replaceState' : 'pushState';
  history[method](history.state, '', url);
}
/** `follow` continues the page already on screen (a new launch reaching its
 *  native history): keep the composer and its focus, and leave the launch
 *  stage up until the conversation replaces it instead of flashing a spinner. */
async function openSession(uid, agent = null, {exact = false, historyMode = 'push', follow = false} = {}) {
  const selectedAgent = agent || null;
  if (!selectedAgent) uid = followContinuedSession(uid);
  if (!selectedAgent && !exact && hiddenForkParent(catalog.sessions.find(s => s.uid === uid))) {
    uid = forkLeafUid(uid);
  }
  browserAuditEvent('session.opened', {agent: selectedAgent || '', cached: cache.has(viewKey(uid, selectedAgent))},
    null, {uid});
  showMobileDetail();
  requests.inflight?.abort();            // 连点列表时, 放弃上一个还没回来的请求
  const ac = requests.inflight = new AbortController();
  closeWatch();
  rendering.cancel();                 // Cancel detached render batches before the next fetch completes.
  rendering.clearSyntax();
  rendering.clearFormulae();
  if (typeof terminal.state !== 'undefined') {
    if (terminal.state.uid && (terminal.state.uid !== uid || selectedAgent)) terminal.closeTermPane(true);
    if (!follow) composer.hide();    // 先收起, 渲染完再按新会话的状态决定
  }
  selection.sel = uid;
  selection.agent = selectedAgent;
  syncSessionStopNotice();
  clearUnread(uid);
  store.set('sel', uid);
  store.set('agent', selection.agent ? { uid, id: selection.agent } : null);
  revealSessionInSidebar(uid, selectedAgent);
  paintSidebarSelection(uid, selectedAgent);
  updateSessionUrl(uid, selectedAgent, historyMode);

  const key = viewKey(uid, selectedAgent);
  const hit = cacheGet(key);
  if (hit) {
    // 先把缓存立即画出来，但暂不占一个长期 SSE 连接。补齐缓存游标之后再
    // 建 watch，避免 HTTP/1 连接池紧张时增量 fetch 永远排在 EventSource 后。
    await renderSession(hit.meta, hit.msgs, hit.activity, { startWatch: false });
    if (selection.sel === uid && selection.agent === selectedAgent) {
      await syncSession(uid, selectedAgent);
      if (selection.sel === uid && selection.agent === selectedAgent) watchSession(uid, selectedAgent);
    }
    return;
  }

  if (!follow) {
    showDetailState('spin','正在读取会话…');
    ensureConsolePlaceholder();
    auditDetailRendered('loading');
  }
  progress(0, 0, '下载');
  let res;
  try {
    res = await fetchMessages(uid, {
      agent: selectedAgent, signal: ac.signal,
      windowed: true,
      onProgress: (a, b, detail) => progress(a, b, '下载', detail),
    });
  } catch (e) {
    if (e.name === 'AbortError') return;      // 已经切到别的会话了
    if (capabilities.config.backend === 'rust'
        && (selection.sel !== uid || selection.agent !== selectedAgent)) return;
    progressDone();
    showDetailState('empty',`读取失败: ${e.message}`);
    ensureConsolePlaceholder();
    if (!reportReadFailure(uid, selectedAgent, e)) scheduleOpenRetry(uid, selectedAgent, ac);
    auditDetailRendered('load-failed', {error: String(e.message || e).slice(0, 300)});
    return;
  }
  if (selection.sel !== uid || selection.agent !== selectedAgent) return; // 期间切了别的视图
  const { data, bytes } = res;
  if (capabilities.config.backend === 'rust') { migrationReadFailures.delete(key); openRetries.delete(key); }
  cachePut(key, { meta: data.meta, msgs: data.messages, version: data.version,
                  end: data.end, anchor: data.anchor, activity: data.activity, bytes,
                  prompt: data.prompt || null, cli: data.cli ?? null,
                  total: data.message_total, partial: data.partial || null });
  unread.cursors.set(key, {end: data.end, head: data.version.head, anchor: data.anchor});
  await renderSession(data.meta, data.messages, data.activity);
}

  return {requests, openRetries, followContinuedSession, scheduleOpenRetry, followSelectedFork, sessionUrl, updateSessionUrl, openSession};
}
