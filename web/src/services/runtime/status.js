const $ = selector => document.querySelector(selector);

import * as Search from '../../migration/search'
import * as Sidebar from '../../migration/sidebar'

import * as SessionUi from '../../migration/session-ui'

import {viewKey} from '../../domain/runtime/messages.js'

export function createStatus({terminal,dom,composer,core,sidebarView,status,sessionUi,conversationRenderer,sidebarGestures,bulk,filters,capabilities,presentation}) {
const agentRunning = (uid,item) => !!item.active && core.state.live.live.has(uid);
function sessionFrozen(uid) {
  return !!uid && typeof terminal().state !== 'undefined' && (terminal().state.list || []).some(row => !row.stale
    && row.frozen === true && (row.uid === uid || row.current_uid === uid || `tmux:${row.name}` === uid));
}

function sessionInputAttention(uid) {
  if (!uid || (typeof composer().sessionComposerEnded === 'function' && composer().sessionComposerEnded(uid))) return '';
  const draft = typeof composer().composerDrafts !== 'undefined'
    ? composer().composerDrafts.get(composer().composerDraftOwner(uid)) : null;
  const cli = core.cache.cache.get(uid)?.cli;
  const current = typeof composer().composerDrafts !== 'undefined'
    && composer().composerDraftOwner(uid) === composer().composerDraftOwner(composer().composerUid);
  const input = current ? draft?.inputStatus || cli?.input : cli?.input;
  // Normal startup/screen synchronization/paste is not a request for help.
  if (input?.state === 'starting' || ['input_check_pending', 'cli_starting',
      'cli_catching_up', 'cli_pasting'].includes(input?.code)) return '';
  const turn = sessionTurn(uid);
  if (input?.code === 'cli_question' || turn === 'waiting') return 'question';
  return '';
}

const inputAttentionLabel = attention => attention === 'question' ? ' · 等待回答' : '';

function paintItemStatus(node) {if (node) sidebarView().refreshSidebarRows(node.dataset.uid);}

function sessionTurn(uid) {
  if (!uid || !core.state.live.live.has(uid)) return '';
  const entry = core.state.selection.sel === uid ? core.cache.cache.get(viewKey(uid)) : null;
  const row = core.index.indexedSessions().byUid.get(uid);
  // 列表 turn 与左栏同一规则；对话 activity 只补上更早到达的"等待回答"。
  let state = entry?.activity?.state === 'waiting' ? 'waiting' : row?.turn || '';
  const busy = entry?.cli?.instance?.busy;
  if (state !== 'waiting' && typeof busy === 'boolean') state = busy ? 'working' : 'idle';
  // 主回合结束但后台子代理或后台任务（Monitor、后台命令）还在跑：会话在等它们，仍算轮转中。
  if (state !== 'waiting' && (core.state.live.liveWorking.has(uid) || row?.background > 0 || (row?.agent_items || []).some(item => status().agentRunning(uid, item)))) state = 'working';
  return ['working', 'waiting'].includes(state) ? state : (state ? 'idle' : '');
}

const turnLabel = turn => ({working: ' · 正在处理', waiting: ' · 等待回答', idle: ' · 空闲'})[turn] || '';

function paintTurn(uid) {
  paintItemStatus(document.querySelector(`.item[data-uid="${CSS.escape(uid)}"]`));
  if (uid === core.state.selection.sel) paintHeaderTurn();
}

function initializeHeaderStatus(meta,pending) {
 const uid=meta.uid,live=pending?true:core.state.live.live.has(uid),tmux=pending?true:core.state.live.liveTmux.has(uid),frozen=sessionFrozen(uid),turn=pending?'':sessionTurn(uid);
 Object.assign(presentation.headerLive,{marker:undefined,visible:live||frozen,tmux,frozen,attention:'',turn:frozen?'':turn,title:frozen?'会话已暂停':dom().liveStatusTitle(tmux)+turnLabel(turn)});
}

function paintHeaderTurn() {
  sessionUi().syncSessionFreezeOverlay();
  const h = $('#dlive');
  if (!h) return;
  const row = document.querySelector(`.item[data-uid="${CSS.escape(core.state.selection.sel || '')}"]`);
  const tmux = row ? row.classList.contains('live-tmux') : core.state.live.liveTmux.has(core.state.selection.sel);
  const frozen = sessionFrozen(core.state.selection.sel);
  const draft = typeof composer().composerDrafts !== 'undefined'
    ? composer().composerDrafts.get(composer().composerDraftOwner(core.state.selection.sel)) : null;
  const active = (row ? row.classList.contains('live') : core.state.live.live.has(core.state.selection.sel))
    || (draft?.cli?.instance?.running === true && !!terminal().takenOver(core.state.selection.sel));
  const attention = !frozen && active ? sessionInputAttention(core.state.selection.sel) : '';
  const turn=frozen || row?.dataset.tmuxName?'':sessionTurn(core.state.selection.sel);
  Object.assign(presentation.headerLive,{marker:`${frozen}:0:${attention}`,visible:frozen||active,tmux,frozen,attention,turn:turn==='working'&&attention?'':turn,title:frozen?'会话已暂停':dom().liveStatusTitle(tmux)+turnLabel(turn)+(attention&&turn!=='waiting'?inputAttentionLabel(attention):'')});
}

function paintLive() {
  for (const group of Sidebar.currentGroups()) for (const view of group.rows) {
    const session = view.row.s;
    if (session.tmuxName) sidebarView().pendingSidebarLive.set(session,
      typeof terminal().state !== 'undefined' && !!terminal().state.list?.some(row => row.name === session.tmuxName && !row.stale));
  }
  sidebarView().refreshSidebarRows();
  const h = $('#dlive');
  if (h) {
    paintHeaderTurn();
  }
  const selected = core.state.catalog.sessions.find(x => x.uid === core.state.selection.sel);
  if (selected) {
    sessionUi().renderSessionAction(selected);
    conversationRenderer().renderConversationTail(core.cache.cache.get(viewKey(selected.uid, core.state.selection.agent))?.activity, selected.uid);
  }
  sessionUi().paintTransferAvailability($('#a-clone-group'), core.state.selection.sel);
  if (sidebarGestures().menuUid) sessionUi().paintTransferAvailability($('#item-menu [data-act="clone"]'), sidebarGestures().menuUid);
  renderSessionCounts();
  syncActiveOnlyList();
  if (core.state.sidebar.picking) bulk().renderPickBar();
}

function syncActiveOnlyList() {
  if (!core.state.sidebar.activeOnly) return;
  const wanted = new Set(filters().visible().map(s => s.uid));
  const shown = $('#side')?._sessionUids || new Set();
  if (wanted.size === shown.size && [...wanted].every(uid => shown.has(uid))) return;
  const side = $('#side'), top = side?.scrollTop || 0;
  sidebarView().renderSide();
  if (side) side.scrollTop = top;
}

function selectSessionScope(activeOnly) {
  if (activeOnly && !capabilities.allows('live')) {
    SessionUi.consoleToast('运行状态未知：Rust 后端尚未实现进程探测，不能按活跃状态筛选。');
    return;
  }
  core.state.sidebar.activeOnly = activeOnly;
  core.preferences.set('activeOnly', core.state.sidebar.activeOnly);
  sidebarView().renderSide();
  paintLive();
}

function sidebarScopeKeydown(e) {
  if (!['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End'].includes(e.key)) return;
  e.preventDefault();
  const active = e.key === 'Home' ? true : e.key === 'End' ? false : !core.state.sidebar.activeOnly;
  selectSessionScope(active);
  $(active ? '#livecount' : '#allcount').focus();
}

function renderSessionCounts() {
  sidebarView().ensureSidebarVue();
  const pool = core.index.sidebarSessions().filter(s => !core.state.sidebar.off.has(s.source) && core.nodes.nodeSelected(s));
  const active = pool.filter(s => s.pending || core.state.live.live.has(s.uid)).length;
  Sidebar.updateCounts(active, pool.length, capabilities.allows('live'), core.state.sidebar.activeOnly, selectSessionScope, sidebarScopeKeydown);
}

function showSessionCount() {
  renderSessionCounts();
  Search.status.textContent = '';
}
function start(){
setInterval(() => { if (!core.events.ready) core.live.pollLive(); }, 3000);
document.addEventListener('visibilitychange', () => {
  if (document.hidden) { core.sync.closeWatch(); return; }
  core.live.pollLive();
  if (core.state.selection.sel && core.cache.cache.get(viewKey(core.state.selection.sel, core.state.selection.agent))) {
    core.sync.syncSession(core.state.selection.sel, core.state.selection.agent).then(() => core.sync.watchSession(core.state.selection.sel, core.state.selection.agent));
  }
});
}
return {sessionFrozen,sessionInputAttention,inputAttentionLabel,paintItemStatus,sessionTurn,turnLabel,paintTurn,initializeHeaderStatus,paintHeaderTurn,paintLive,syncActiveOnlyList,selectSessionScope,sidebarScopeKeydown,renderSessionCounts,showSessionCount,agentRunning,start};
}
