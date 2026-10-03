// Catalog generations, cancellation and poll deduplication belong to the service.
export function createSessionList(environment, state, nodes, index, unread, presentation) {
  const {network, fetch, appUrl} = environment;
  const {catalog, selection, unread: unreadState, search} = state;
  const {applyNodeState, renderNodes} = nodes;
  const {hiddenForkParent, sidebarSessions} = index;
  const {seedSidebarCursors, syncSidebarUpdates} = unread;
  const {status, ensureSidebarVue, loadFailed, refreshSessionMeta, renderChips, renderSide,
    showSessionCount, followSelectedFork, patchSide, visible, paintLive} = presentation;
  const $ = selector => document.querySelector(selector);
let sessionLoadRun = 0;
let sessionLoadRetry = null;
let sessionPollRequest = null, sessionPollController = null, sessionLoadActive = 0;

async function loadSessions(force) {
  if (network.paused) return false;
  const run = ++sessionLoadRun;
  sessionLoadActive = run;
  sessionPollController?.abort();
  clearTimeout(sessionLoadRetry);
  status.textContent = force ? ' 重新扫描…' : ' 加载中…';
  const ac = new AbortController();
  const timeout = setTimeout(() => ac.abort(), 15000);
  let d;
  try {
    const r = await fetch(appUrl('api/sessions' + (force ? '?force=1' : '')), { signal: ac.signal });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    d = await r.json();
    if (!Array.isArray(d.sessions)) throw new Error('会话列表格式错误');
  } catch (e) {
    if (run !== sessionLoadRun || network.paused) return false;
    status.textContent = ' 加载失败';
    status.classList.add('err');
    ensureSidebarVue(); loadFailed(() => loadSessions(false));
    sessionLoadRetry = setTimeout(() => loadSessions(false), 3000);
    return false;
  } finally {
    clearTimeout(timeout);
    if (sessionLoadActive === run) sessionLoadActive = 0;
  }
  if (run !== sessionLoadRun) return false;
  status.classList.remove('err');
  const seedCursors = unreadState.cursors.size === 0;
  const wasListed = !hiddenForkParent(catalog.sessions.find(s => s.uid === selection.sel));
  catalog.sig = d.sig;
  catalog.sessions = d.sessions;
  applyNodeState(d, 'sessions');
  refreshSessionMeta();
  renderChips();
  renderSide();
  showSessionCount(sidebarSessions().length);
  seedCursors ? seedSidebarCursors(d.sessions) : syncSidebarUpdates(d.sessions);
  if (wasListed) void followSelectedFork();
  return true;
}

/** 列表自动跟进磁盘变化。签名没变时服务端只回一个 unchanged, 成本为个位数毫秒。 */
function pollSessions() {
  if (network.paused) return Promise.resolve();
  if (document.hidden) return Promise.resolve();
  if (!catalog.sig || sessionLoadActive) return Promise.resolve(false);
  if (sessionPollRequest) return sessionPollRequest;
  sessionPollRequest = runSessionPoll().finally(() => { sessionPollRequest = null; });
  return sessionPollRequest;
}
async function runSessionPoll() {
  const run = sessionLoadRun, sig = catalog.sig;
  const ac = new AbortController();
  sessionPollController = ac;
  const timeout = setTimeout(() => ac.abort(), 15000);
  try {
    const response = await fetch(appUrl('api/sessions?sig=' + encodeURIComponent(sig)), {signal: ac.signal});
    if (!response.ok) return false;
    const d = await response.json();
    if (ac.signal.aborted || run !== sessionLoadRun || sig !== catalog.sig) return false;
    applyNodeState(d, 'sessions');
    if (d.unchanged || !d.sessions) return true;
    const wasListed = !hiddenForkParent(catalog.sessions.find(s => s.uid === selection.sel));
    catalog.sig = d.sig;
    catalog.sessions = d.sessions;
    renderNodes();
    refreshSessionMeta();
    renderChips();
    syncSidebarUpdates(d.sessions);
    if (wasListed) await followSelectedFork();
    if (search.results) {
      // 搜索结果集合保持不变，只合入 rename 等最新元数据。
      const fresh = new Map(catalog.sessions.map(s => [s.uid, s]));
      search.results = search.results.map(r => {
        const latest = fresh.get(r.uid);
        if (!latest) return r;
        const agents = new Map((latest.agent_items || []).map(a => [a.id, a]));
        return {...r, ...latest, agent_items: (r.agent_items || []).map(a => ({
          ...a, ...agents.get(a.id), hits: a.hits, hits_capped: a.hits_capped, snippet: a.snippet,
        }))};
      });
      if (!patchSide(visible())) renderSide();
      return true;
    }
    showSessionCount(sidebarSessions().length);
    if (patchSide(visible())) return true;   // 能就地更新就不重建, 否则会一直闪
    const side = $('#side');
    const top = side.scrollTop;
    renderSide();                       // 选中态由 selection.sel 恢复
    side.scrollTop = top;               // 别打断正在看的位置
    paintLive();
    return true;
  } catch { return false; }
  finally {
    clearTimeout(timeout);
    if (sessionPollController === ac) sessionPollController = null;
  }
}

  function pause() {
    sessionPollController?.abort();
    clearTimeout(sessionLoadRetry);
  }
  return {loadSessions, pollSessions, runSessionPoll, pause,
    get polling() {return sessionPollRequest;}};
}
