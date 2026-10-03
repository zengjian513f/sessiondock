export function createLiveService(environment, catalog, state, nodes, terminal, presentation, trimCache) {
  const {capabilities, network, fetch, appUrl} = environment;
  const {applyNodeState} = nodes;
  const {paintLive, renderTakeoverBtn} = presentation;
async function refreshLive(force = false) {
  if (!capabilities.allows('live')) return;
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 12000);
  let d;
  try {
    const response = await fetch(appUrl('api/live' + (force ? '?force=1' : '')), {signal:controller.signal});
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    d = await response.json();
  } finally { clearTimeout(timeout); }
  if (!Array.isArray(d.uids)) throw new Error('运行状态格式错误');
  applyNodeState(d, 'live');
  const next = new Set(d.uids);
  const nextTmux = new Set((d.tmux_uids || []).filter(u => next.has(u)));
  const nextWorking = new Set((d.working_uids || []).filter(u => next.has(u)));
  // Work another machine runs for a session, named by native node/source/sid.
  const remoteKey = (node, source, sid) => JSON.stringify([node || '', source, String(sid)]);
  const remote = new Set((Array.isArray(d.remote_working) ? d.remote_working : [])
    .map(r => remoteKey(r.node_id, r.source, r.sid)));
  if (remote.size) for (const s of catalog.sessions || []) {
    if (next.has(s.uid) && remote.has(remoteKey(s.node_id, s.source, s.sid))) nextWorking.add(s.uid);
  }
  const nextStarted = new Map(Object.entries(d.started_at || {}).map(([u, t]) => [u, +t]));
  const setChanged = (a, b) => a.size !== b.size || [...a].some(u => !b.has(u));
  const mapChanged = (a, b) => a.size !== b.size
    || [...a].some(([u, t]) => b.get(u) !== t);
  const changed = setChanged(next, state.live) || setChanged(nextTmux, state.liveTmux)
    || setChanged(nextWorking, state.liveWorking)
    || mapChanged(nextStarted, state.liveStarted);
  state.live = next;
  state.liveTmux = nextTmux;
  state.liveWorking = nextWorking;
  state.liveStarted = nextStarted;
  trimCache();
  if (changed) {
    paintLive();
  }
}

let livePollRequest = null;
function pollLive(force = false) {
  if (network.paused) return Promise.resolve();
  if (!capabilities.allows('live')) return Promise.resolve();
  if (document.hidden) return Promise.resolve();
  if (livePollRequest) {
    // Timer ticks share the current cycle; explicit refreshes still get a
    // fresh snapshot after it, and awaiters never observe a half-finished poll.
    return force ? livePollRequest.then(() => pollLive(true)) : livePollRequest;
  }
  livePollRequest = runLivePoll(force).finally(() => { livePollRequest = null; });
  return livePollRequest;
}

async function runLivePoll(force) {
  try {
    await refreshLive(force);
    if (typeof terminal.loadTermList === 'function') {   // tmux 会话可能在外部被结束
      // 首屏：term.js 自己已经发出了列表请求，复用它；列表从"未加载"变为有内容
      // 不算变化，否则每次打开页面都会多一轮强制 live 刷新。刚返回不到 1 秒的
      // 列表也不重拉（首屏 live 与 term.js 的初始请求前后脚到达）；显式强刷除外。
      const first = !terminal.state.listLoaded;
      const before = (terminal.state.list || []).map(x => x.name).join();
      const fresh = !force && terminal.state.listLoadedAt && performance.now() - terminal.state.listLoadedAt < 1000;
      if (first && terminal.state.listRequest) await terminal.state.listRequest;
      else if (!fresh) await terminal.loadTermList();
      if (!first && (terminal.state.list || []).map(x => x.name).join() !== before) {
        await refreshLive(true);                // 绕过 3 秒缓存，绿点立即跟着 tmux 消失
        renderTakeoverBtn();
        if (terminal.state.name && !terminal.state.list.some(x => x.name === terminal.state.name)) terminal.closeTermPane();
      }
    }
  } catch { /* 服务端没起来就下轮再说 */ }
}

/** 只改小圆点, 不重渲染整个列表 —— 否则每几秒就会打断滚动和选中。 */
  return {refreshLive, pollLive, runLivePoll};
}
