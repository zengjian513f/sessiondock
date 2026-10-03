const $ = selector => document.querySelector(selector);
import * as Shell from '../../migration/shell'
import * as Search from '../../migration/search'
import * as Sidebar from '../../migration/sidebar'

import * as SessionUi from '../../migration/session-ui'

import {SOURCES} from '../../domain/runtime/sources'

import {fmtSize,fmtTime,fmtSpan,shortCwd} from '../../domain/runtime/format.js'

export function createSidebarView({core,status,bulk,terminal,sidebarGestures,metadata,timeline,composer,filters,dom,capabilities,resources,sidebarResources}) {
function applySourceFilterChange() {
  core.preferences.set('off', [...core.state.sidebar.off]);
  renderChips(); controller.renderSide();
  if (core.environment.HUB_MODE && core.state.search.results !== null) void Search.runSearch();
}

function selectOnlySource(source) {
  if (!Object.hasOwn(SOURCES, source)) return false;
  const control = document.querySelector(`#chips button[data-source="${CSS.escape(source)}"]`);
  if (control?.dataset.unavailableReason) { return false; }
  core.state.sidebar.off = new Set(Object.keys(SOURCES).filter(item => item !== source));
  applySourceFilterChange();
  return true;
}

function selectOnlyNodeFilter(id) {
  const node = core.state.nodes.list.find(item => item.id === id);
  if (!node) return false;
  if (node.online === false) { return false; }
  core.state.nodes.off = new Set(core.state.nodes.list.filter(item => item.id !== id).map(item => item.id));
  core.preferences.set('nodesOff', [...core.state.nodes.off]);
  core.nodes.renderNodes(); renderChips(); controller.renderSide();
  status().showSessionCount(core.index.sidebarSessions().filter(core.nodes.nodeSelected).length);
  if (core.state.search.results !== null) void Search.runSearch();
  return true;
}

function renderChips() {
  ensureSidebarVue(); const counts = new Map();
  for (const row of core.index.sidebarSessions()) if (core.nodes.nodeSelected(row)) counts.set(row.source, (counts.get(row.source) || 0) + 1);
  Sidebar.updateSources(Object.entries(SOURCES).map(([key, value]) => ({
    key, label: value.name, count: counts.get(key) || 0, on: !core.state.sidebar.off.has(key), icon: value.icon, color: value.color,
    title: `${value.name}：点击选择或取消；右键或长按只选此类型`,
    reason: !counts.get(key) ? `${value.name} 在当前选择的机器上没有会话。` : ''})));
}

function selectOnlyFilter(button) {
  if (button.dataset.unavailableReason) return false;
  if (button.dataset.node) return selectOnlyNodeFilter(button.dataset.node);
  if (button.dataset.source) return selectOnlySource(button.dataset.source);
  return false;
}

const spawnKey = (nodeId, source, sid) => JSON.stringify([nodeId || '', source, String(sid)]);

function nestSpecParent(session) {
  return session.nest_parent?.source && session.nest_parent.sid
    ? {parent: session.nest_parent} : null;
}

function nestParentOf(session, byKey, allByKey = new Map(core.state.catalog.sessions.map(s => [spawnKey(s.node_id, s.source, s.sid), s]))) {
  return Sidebar.Grouping.parentOf(session, byKey, allByKey, sidebarGroupingContext());
}

function nestEdges(list) {return Sidebar.Grouping.nestEdges(list, sidebarGroupingContext());}

function nestTree(list) {return core.state.sidebar.nest ? nestEdges(list) : {children: new Map(), nested: new Set()};}

function nestDescendantUids(uid, list = core.index.sidebarSessions()) {
  const {children} = nestEdges(list);
  const out = new Set();
  const walk = id => {
    for (const child of children.get(id) || []) {
      if (out.has(child.uid)) continue;
      out.add(child.uid);
      walk(child.uid);
    }
  };
  walk(uid);
  return out;
}

function applySessionNest(uid, nestParent) {
  for (const rows of [core.state.catalog.sessions, core.state.search.results || []]) {
    const row = rows.find(session => session.uid === uid);
    if (!row) continue;
    if (nestParent) row.nest_parent = nestParent;
    else delete row.nest_parent;
  }
}

function setNestAttach(value) {
  core.state.sidebar.nestAttachUids = (Array.isArray(value) ? value : [value]).filter(Boolean);
  core.state.sidebar.nestAttach = core.state.sidebar.nestAttachUids[0] || '';
  if (core.state.sidebar.nestAttach && core.state.sidebar.picking) {
    core.state.sidebar.picking = false;
    bulk().state.picked.clear();
  }
  const side = $('#side'), top = side.scrollTop;
  bulk().renderPickBar();
  controller.renderSide();
  side.scrollTop = top;
}

async function setSessionNest(uid, {parent_uid = null} = {}) {
  try {
    const response = await core.network.fetch(core.environment.appUrl('api/session/nest'), {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({uid, parent_uid}),
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(data.error || `HTTP ${response.status}`);
    applySessionNest(uid, data.nest_parent || null);
    if (data.nest_parent && !core.state.sidebar.nest) {
      core.state.sidebar.nest = true;
      core.preferences.set('nest', true);
      renderView();
    }
    const side = $('#side'), top = side?.scrollTop || 0;
    controller.renderSide();
    if (side) side.scrollTop = top;
    return data;
  } catch (error) {
    await SessionUi.appAlert('会话附属关系保存失败: ' + error.message);
    return null;
  }
}

async function pickNestParent(target) {
  const listed = new Set(core.index.sidebarSessions().map(session => session.uid));
  const uids = core.state.sidebar.nestAttachUids.filter(uid => listed.has(uid));
  if (!uids.length) { setNestAttach(''); return; }
  if (!target?.uid || uids.includes(target.uid)) return;
  if (uids.some(uid => nestDescendantUids(uid).has(target.uid))) {
    await SessionUi.appAlert('不能附属到自己的子会话下面');
    return;
  }
  // 逐条保存；失败的留在点选里（setSessionNest 已提示原因），可以再点一次。
  const failed = [];
  for (const uid of uids) {
    if (!await setSessionNest(uid, {parent_uid: target.uid})) failed.push(uid);
  }
  setNestAttach(failed);
}

function nestStamp(s, children, memo = new Map()) {return Sidebar.Grouping.nestStamp(s, children, memo);}

const sidebarGroupFolds = () => core.state.search.term ? core.state.search.searchClosed : core.state.sidebar.closed;

const sidebarNestFolds = () => core.state.search.term ? core.state.search.searchNestClosed : core.state.sidebar.nestClosed;

const sidebarGroupClosed = key => sidebarGroupFolds().has(key);

const sidebarNestClosed = uid => sidebarNestFolds().has(uid);

function sidebarSearchText(s, agent = null) {
  const row = agent || s;
  return [row.title, row.cwd || s.cwd, s.node_name, s.source, SOURCES[s.source]?.name,
    row.model, agent ? agent.type : s.agent_type,
    ...(agent ? [agent.id] : [s.sid, s.uid])].join('\n');
}

function sidebarMainMatches(s) {
  if (!core.state.search.term) return true;
  return core.state.search.results !== null ? s.hits > 0
    : Search.matchesSearch(sidebarSearchText(s));
}

function sidebarAgentItems(s) {
  const agents = s.agent_items || [];
  if (!core.state.search.term) return agents;
  if (core.state.search.results !== null) return agents.filter(a => a.hits > 0);
  return agents.filter(a => Search.matchesSearch(sidebarSearchText(s, a)));
}

function sidebarMatchCount(list) {
  return list.reduce((count, s) => count + Number(sidebarMainMatches(s)) + sidebarAgentItems(s).length, 0);
}

function sidebarRowSnippet(s, agent = null) {
  return agent ? agent.snippet : sidebarMainMatches(s) ? s.snippet : '';
}

function nestSize(s, children, memo = new Map()) {return Sidebar.Grouping.nestSize(s, children, sidebarGroupingContext(), memo);}

function expandRows(s, depth, children, out, seen, memo = new Map(), sizes = new Map()) {
  return Sidebar.Grouping.expandRows(s, depth, children, out, seen, sidebarGroupingContext(), memo, sizes);
}

const rowKey = row => Sidebar.Grouping.rowKey(row);

function groupBy(list, {skipClosed = false} = {}) {return Sidebar.Grouping.groupBy(list, sidebarGroupingContext(), skipClosed);}

const pendingMeta = s => `${fmtTime(s.updated)} · ${typeof terminal().pendingStateLabel === 'function'
  ? terminal().pendingStateLabel(s) : '等待首条消息'}`;

const rustPendingRow = s => !!s.pending && !!s.record_id && capabilities.config.backend === 'rust';

const forkMeta = s => {
  if (s.agent_id) return '';
  const branch = s.forked_from_id
    ? (Number.isInteger(s.fork_depth) && s.fork_depth > 0 ? `分叉 ${s.fork_depth}` : '分叉会话')
    : '';
  return s.fork_parent ? `父会话（${branch || '原始'}）` : branch;
};

const itemMeta = s => (s.stale && !rustPendingRow(s) ? '离线缓存 · ' : '') + (s.pending ? pendingMeta(s)
  : [forkMeta(s), fmtTime(s.updated), fmtSize(s.size), s.model || '',
                       s.hits ? `命中 ${s.hits}${s.hits_capped ? '+' : ''}` : '']
                      .filter(Boolean).join(' · '));

function patchSide(list) {
  controller.renderSide(list);
  return true;
}

const agentMeta = (uid, a) => `子代理 · ${a.type} · ${fmtSpan(a.created, status().agentRunning(uid, a) ? null : a.updated)}`
  + (core.state.search.term && a.hits > 0 ? ` · 命中 ${a.hits}${a.hits_capped ? '+' : ''}` : '');

function paintAgentStatus(node) {if (node) refreshSidebarRows(node.dataset.owner);}

function paintSidebarSelection(uid, agent = null) {
  const side = $('#side');
  const row = agent ? side.querySelector(`.item.agent[data-owner="${CSS.escape(uid)}"][data-agent="${CSS.escape(agent)}"]`) : side.querySelector(`.item[data-uid="${CSS.escape(uid)}"]`);
  if (!row) {const top = side.scrollTop; controller.renderSide(); side.scrollTop = top; return false;}
  refreshSidebarRows(); return true;
}

var sidebarVueMounted;

function ensureSidebarVue() {
  if (sidebarVueMounted) return;
  sidebarVueMounted = true;
  Sidebar.mount({
    rowClick: (r, event) => {
      if (sidebarGestures().sidebarTextSelectionActive()) { event.preventDefault(); return; }
      if (r.agent) { if (!core.state.sidebar.nestAttach && !core.state.sidebar.picking) core.open.openSession(r.s.uid, r.agent.id); return; }
      if (core.state.sidebar.nestAttach) { void pickNestParent(r.s); return; }
      if (core.state.sidebar.picking) { if (bulk().sessionPickable(r.s)) bulk().toggleSessionPick(r.s.uid); return; }
      r.s.pending ? terminal().openPendingSession(r.s) : core.open.openSession(r.s.uid);
    },
    star: metadata().toggleSessionStar, nestFold: (...args)=>controller.toggleNestFold(...args),
    groupFold: (key, event) => {
      if (sidebarGestures().sidebarTextSelectionActive()) { event.preventDefault(); return; }
      const folds = sidebarGroupFolds(); folds.has(key) ? folds.delete(key) : folds.add(key);
      if (!core.state.search.term) core.preferences.set('closed', [...core.state.sidebar.closed]); controller.renderSide();
    },
    groupPick: bulk().toggleGroupPick, createGroup: input => core.groups.create(input),
    removeGroup: name => core.groups.remove(name),
    editGroup: on => core.groups.edit(on), exitSearch: Search.exitSearch,
    pickAll: bulk().pickAllVisible, pickStop: bulk().stopPickedSessions,
    pickGroup: event => core.groups.showMenu([...bulk().state.picked], event.currentTarget),
    pickAttach: () => setNestAttach(bulk().pickedNestable()), pickDelete: bulk().deletePickedSessions,
    pickCancel: () => core.state.sidebar.nestAttach ? setNestAttach('') : bulk().setPicking(false),
    sourceClick: key => { core.state.sidebar.off.has(key) ? core.state.sidebar.off.delete(key) : core.state.sidebar.off.add(key); applySourceFilterChange(); },
    nodeClick: id => { const n = core.state.nodes.list.find(n => n.id === id); if (!n || n.online === false) return;
      core.state.nodes.off.has(id) ? core.state.nodes.off.delete(id) : core.state.nodes.off.add(id); core.preferences.set('nodesOff', [...core.state.nodes.off]);
      core.nodes.renderNodes(); renderChips(); controller.renderSide(); status().showSessionCount(); if (core.state.search.results !== null) void Search.runSearch(); },
    resourceOpen: session => resources()?.open(session),
    resourceCells: (session, agent) => sidebarResources().cells(session, agent),
    rowMounted: Sidebar.observeRow, rowUnmounted: Sidebar.unobserveRow,
  }, sidebarGestures().sidebarVueGestures(), {selectOnly: selectOnlyFilter, holdMs: sidebarGestures().LONG_PRESS_MS, slop: sidebarGestures().LONG_PRESS_SLOP});
}

function sidebarGroupingContext() {
  return {view: core.state.sidebar.view, nest: core.state.sidebar.nest, term: core.state.search.term, picking: core.state.sidebar.picking, live: core.state.live.live, sessions: core.state.catalog.sessions,
    groupClosed: sidebarGroupClosed, nestClosed: sidebarNestClosed, mainMatches: sidebarMainMatches,
    agents: sidebarAgentItems, agentRunning:status().agentRunning, pickable: bulk().sessionPickable, dayKey:timeline().dayKey,
    names: core.groups?.names || [], available: !!core.groups?.available,
    contains: name => core.groups.contains(name), continued: core.index.sessionContinued,
    byUid: uid => core.index.indexedSessions().byUid.get(uid), expand: (...args) => controller.expandRows(...args), hiddenForkParent:core.index.hiddenForkParent, forkChildren:core.index.forkChildren};
}

function sidebarStatus(s, agent = null) {
  if (agent) {
    const running = sidebarAgentRunning(s.uid, agent);
    return Sidebar.Presentation.statusView({agent: true, running});
  }
  const unread = core.status.unreadRow(s.uid);
  const pending = sidebarPendingRunning(s);
  const draft = typeof composer().composerDrafts !== 'undefined' ? composer().composerDrafts.get(composer().composerDraftOwner(s.uid)) : null;
  const active = pending || core.state.live.live.has(s.uid) || (draft?.cli?.instance?.running === true && !!terminal().takenOver(s.uid));
  const tmux = pending || core.state.live.liveTmux.has(s.uid), frozen = status().sessionFrozen(s.uid);
  const attention = !frozen && active ? status().sessionInputAttention(s.uid) : '';
  const turn = frozen || pending ? '' : status().sessionTurn(s.uid);
  return Sidebar.Presentation.statusView({running: active, count: unread.count, pending, tmux, frozen, attention, turn,
    turnLabel: status().turnLabel(turn), attentionLabel: status().inputAttentionLabel(attention)});
}

function sidebarAgentRunning(uid, agent) {
  const item = (core.index.indexedSessions().byUid.get(uid)?.agent_items || []).find(item => item.id === agent.id);
  return !!item && status().agentRunning(uid, item);
}

const pendingSidebarLive = new WeakMap();

function sidebarPendingRunning(s) {
  // New receipt rows start live before term/list catches up. Subsequent live
  // paints follow that list, as the original row class and status marker did.
  return !!s.tmuxName && (pendingSidebarLive.get(s) ?? !s.stale);
}

function sidebarPresentationState() {
  return {view: core.state.sidebar.view, selected: core.state.selection.sel, agent: core.state.selection.agent, picking: core.state.sidebar.picking, live: core.state.live.live, liveTmux: core.state.live.liveTmux,
    picked: bulk().state.picked, starBusy: core.state.sidebar.starBusy, nestAttachUids: core.state.sidebar.nestAttachUids, searching: !!core.state.search.term};
}

function sidebarPresentationHelpers() {
  return {pickable: bulk().sessionPickable, snippet: sidebarRowSnippet, highlight: Search.hl, snippetHtml: Search.sidebarSnippet,
    itemMeta, agentMeta, agentRunning: sidebarAgentRunning, pendingRunning: sidebarPendingRunning, status: sidebarStatus, source: source => SOURCES[source], path: timeline().timelinePath,
    nodeColor:core.nodes.nodeColor, shortCwd:shortCwd, nodeDirectory:core.nodes.nodeDirectory, groupClosed: sidebarGroupClosed, mainMatches: sidebarMainMatches,
    groupsAvailable: !!core.groups?.available, groupContains: name => core.groups.contains(name)};
}

function sidebarRowView(row) {return Sidebar.Presentation.rowView(row, sidebarPresentationState(), sidebarPresentationHelpers());}

function sidebarGroupView(key, rows, summary = undefined) {return Sidebar.Presentation.groupView(key, rows, summary, sidebarPresentationState(), sidebarPresentationHelpers());}

function refreshSidebarRows(uid = null) {
  if (!sidebarVueMounted || sidebarGestures().sidebarTextSelectionProtected()) { sidebarGestures().sidebarRenderDeferred = true; return; }
  Sidebar.repaintRows(sidebarRowView, uid || undefined);
}

function stampSidebarGroups() {
  for (const view of Sidebar.currentGroups()) {
    const group = [...$('#side').children].find(node => node.dataset.key === view.key);
    if (!group) continue;
    group._rows = view.rows.map(row => row.row); group._pickUids = view.pickUids;
    group.querySelector('.ghead')._pickLabel = view.label;
  }
}

function renderSide(suppliedList = null) {
  if (sidebarGestures().sidebarTextSelectionProtected()) {sidebarGestures().sidebarRenderDeferred = true; return;}
  sidebarGestures().sidebarRenderDeferred = false; ensureSidebarVue(); status().renderSessionCounts();
  const side = $('#side'), top = side.scrollTop, list = suppliedList || filters().visible();
  side._sessionUids = new Set(list.map(row => row.uid)); Search.paintSearchMode(list); bulk().syncPickedSessions(); bulk().renderPickBar();
  const groups = controller.groupBy(list, {skipClosed: true});
  const empty = !list.length && !(core.state.sidebar.view === 'group' && core.groups?.available);
  side._nestTree = empty ? null : {children: groups.children, sessions: core.state.catalog.sessions, results: core.state.search.results, context: sidebarNestContext()};
  Sidebar.update({groups: empty ? [] : groups.map(([key, rows, summary]) => sidebarGroupView(key, rows, summary)),
    empty: !empty ? '' : core.state.search.term ? '当前搜索无匹配会话' : core.state.sidebar.activeOnly ? (core.state.search.results ? '没有活动的匹配会话' : '没有活动会话') : (core.state.search.results ? '没有匹配的会话' : '没有会话'),
    searching: !!core.state.search.term, picking: core.state.sidebar.picking, attaching: !!core.state.sidebar.nestAttach,
    createGroup: core.state.sidebar.view === 'group' && !!core.groups?.available, groupMode: core.state.sidebar.view === 'group',
    groupBusy: !!core.groups?.busy, groupEditing: !!core.groups?.editing});
  stampSidebarGroups(); timeline().fitTimelineDirectories(); side.scrollTop = top;
}

function revealSessionInSidebar(uid, agent) {
  const target = agent ? $('#side').querySelector(`.item[data-owner="${CSS.escape(uid)}"][data-agent="${CSS.escape(agent)}"]`)
    : $('#side').querySelector(`.item[data-uid="${CSS.escape(uid)}"]`);
  // Selecting an existing row opens its conversation, not its descendants.
  // In particular, do not clear a fold the user set with this row's caret.
  if (target) {
    target.scrollIntoView({block: 'nearest'});
    return;
  }
  const row = core.state.catalog.sessions.find(s => s.uid === uid);
  if (!row) return;
  let changed = false;
  if (!filters().visible().some(s => s.uid === uid)) {
    if (core.state.search.term || core.state.search.results) Search.cancelSearch(true);
    if (core.state.sidebar.off.delete(row.source)) core.preferences.set('off', [...core.state.sidebar.off]);
    if (core.environment.HUB_MODE && core.state.nodes.off.delete(row.node_id)) core.preferences.set('nodesOff', [...core.state.nodes.off]);
    if (core.state.sidebar.activeOnly) { core.state.sidebar.activeOnly = false; core.preferences.set('activeOnly', false); }
    changed = true;
  }
  // 子代理行两种模式都在，深链不再替用户打开分层开关；分层模式才需要沿发起链展开祖先。
  const byKey = new Map(core.state.catalog.sessions.map(s => [spawnKey(s.node_id, s.source, s.sid), s]));
  const seen = new Set();
  for (let current = row; current && !seen.has(current.uid);
       current = core.state.sidebar.nest ? nestParentOf(current, byKey) : null) {
    seen.add(current.uid);
    // A hidden link target needs its ancestors, but not its own children.
    if ((current.uid !== uid || agent) && sidebarNestFolds().delete(current.uid)) changed = true;
  }
  if (changed && !core.state.search.term) core.preferences.set('nestClosed', [...core.state.sidebar.nestClosed]);
  const groups = controller.groupBy(filters().visible(), {skipClosed: true});
  const contains = session => session.uid === uid
    || (groups.children.get(session.uid) || []).some(contains);
  for (const [key, rows, summary] of groups) {
    const found = summary ? summary.roots.some(contains) : rows.some(r => r.s.uid === uid);
    if (found && sidebarGroupFolds().delete(key)) {
      if (!core.state.search.term) core.preferences.set('closed', [...core.state.sidebar.closed]);
      changed = true;
    }
  }
  if (changed) { renderView(); renderChips(); if (core.environment.HUB_MODE) core.nodes.renderNodes(); controller.renderSide(); }
  const revealed = agent ? $('#side').querySelector(`.item[data-owner="${CSS.escape(uid)}"][data-agent="${CSS.escape(agent)}"]`)
    : $('#side').querySelector(`.item[data-uid="${CSS.escape(uid)}"]`);
  revealed?.scrollIntoView({block: 'nearest'});
}

function renderView() { Shell.updateView(core.state.sidebar.view, core.state.sidebar.nest); }

function sidebarNestContext() {
  return JSON.stringify([core.state.sidebar.view, core.state.sidebar.nest, core.state.sidebar.picking, core.state.sidebar.nestAttachUids, core.state.search.term, core.state.search.opts,
    core.state.sidebar.activeOnly, [...core.state.sidebar.off], core.environment.HUB_MODE ? [...core.state.nodes.off] : []]);
}

function patchNestFold(uid) {
  const side = $('#side'), tree = side?._nestTree;
  if (sidebarGestures().sidebarTextSelectionProtected() || !tree || tree.sessions !== core.state.catalog.sessions || tree.results !== core.state.search.results || tree.context !== sidebarNestContext()) return false;
  const node = side.querySelector(`.item[data-uid="${CSS.escape(uid)}"]`), current = node?._nestRow, group = node?.closest('.group');
  const index = group?._rows?.indexOf(current) ?? -1;
  if (!current || index < 0 || !node.querySelector('.nest-caret')) return false;
  const top = side.scrollTop, rows = [];
  if (sidebarNestClosed(uid)) rows.push({...current, closed: true});
  else controller.expandRows(current.s, current.depth, tree.children, rows, new Set([uid]), new Map());
  let end = index + 1;
  while (end < group._rows.length && group._rows[end].depth > current.depth) end++;
  const delta = rows.length - 1 - (end - index - 1);
  const updated = group._rows.slice(0, index).concat(rows, group._rows.slice(end));
  let depth = current.depth;
  for (let i = index - 1; depth > 0 && i >= 0; i--) {
    const ancestor = updated[i]; if (ancestor.depth >= depth || ancestor.agent) continue;
    depth = ancestor.depth; updated[i] = {...ancestor, kids: ancestor.kids + delta};
  }
  Sidebar.updateGroups(Sidebar.currentGroups().map(view => view.key === group.dataset.key ? sidebarGroupView(view.key, updated) : view));
  stampSidebarGroups();
  const paths = rows.slice(1).flatMap(row => [...side.querySelectorAll(`.item[data-key="${CSS.escape(rowKey(row))}"] .cwd-path`)]);
  if (paths.length) timeline().fitTimelineDirectories(paths, timeline().timelineFitContext);
  side.scrollTop = top; return true;
}

function toggleNestFold(uid) {
  const folds = sidebarNestFolds();
  folds.has(uid) ? folds.delete(uid) : folds.add(uid);
  if (!core.state.search.term) core.preferences.set('nestClosed', [...core.state.sidebar.nestClosed]);
  if (!patchNestFold(uid)) controller.renderSide();
}
function start(){
Search.configure({
  query: () => $('#q').value,
  read: () => ({term: core.state.search.term, opts: core.state.search.opts, results: core.state.search.results, off: core.state.sidebar.off,
    cur: core.state.search.cur, markCapped: core.state.search.markCapped, autoOpen: core.state.search.autoOpen}),
  write: patch => Object.assign(core.state.search, patch),
  clearFolds: () => { core.state.search.searchClosed.clear(); core.state.search.searchNestClosed.clear(); },
  renderSide: () => controller.renderSide(), showSessionCount: () => status().showSessionCount(),
  persistOptions: opts => core.preferences.set('opts', opts),
  fetch: (...args)=>core.network.fetch(...args), allowsSearch: () => capabilities.allows('search'), appUrl:core.environment.appUrl,
  hub: core.environment.HUB_MODE, sources: Object.keys(SOURCES), nodes: () => core.state.nodes.list,
  selectedNodeIds:core.nodes.selectedNodeIds, clearNodeErrors: () => { core.state.nodes.errors.delete('search'); core.nodes.renderNodes(); },
  applyNodeState: data => core.nodes.applyNodeState(data, 'search'),
  count: rows => sidebarMatchCount(rows), escape: dom().esc,
  showMatches: rows => controller.showSearchMatches(rows),
});
}
const controller = {showSearchMatches:Search.showSearchMatches,applySourceFilterChange,selectOnlySource,selectOnlyNodeFilter,renderChips,selectOnlyFilter,spawnKey,nestSpecParent,nestParentOf,nestEdges,nestTree,nestDescendantUids,applySessionNest,setNestAttach,setSessionNest,pickNestParent,nestStamp,sidebarGroupFolds,sidebarNestFolds,sidebarGroupClosed,sidebarNestClosed,sidebarSearchText,sidebarMainMatches,sidebarAgentItems,sidebarMatchCount,sidebarRowSnippet,nestSize,expandRows,rowKey,groupBy,pendingMeta,rustPendingRow,forkMeta,itemMeta,patchSide,agentMeta,paintAgentStatus,paintSidebarSelection,get sidebarVueMounted(){return sidebarVueMounted},ensureSidebarVue,sidebarGroupingContext,sidebarStatus,sidebarAgentRunning,pendingSidebarLive,sidebarPendingRunning,sidebarPresentationState,sidebarPresentationHelpers,sidebarRowView,sidebarGroupView,refreshSidebarRows,stampSidebarGroups,renderSide,revealSessionInSidebar,renderView,sidebarNestContext,patchNestFold,toggleNestFold,start};
return controller;
}
