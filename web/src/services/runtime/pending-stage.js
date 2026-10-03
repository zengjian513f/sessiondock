import * as SessionUi from '../../migration/session-ui'
import * as Conversation from '../../migration/conversation'
import * as Search from '../../migration/search'
import * as Shell from '../../migration/shell'
import {SOURCES} from '../../domain/runtime/sources'
import {pendingUid} from '../../domain/runtime/pending'

export function createPendingStage({terminal,core,conversationRenderer,composer,pendingStage,status,sessionUi,takeover,sidebarView,capabilities}){
function showNewSessionStage(info) {
  if (!terminal().state.pendingModes.has(info.name)) terminal().state.pendingModes.set(info.name, terminal().state.mode);
  // 临时会话也必须完整切换视图；列表或字体慢时不能继续显示/操作旧终端。
  core.open.requests.inflight?.abort();
  core.sync.closeWatch();
  terminal().closeTermPane(true);
  conversationRenderer().progressDone();
  // create 返回后 term/list 可能还没拉完；先把服务端刚确认的新 tmux 放进本地
  // pending，详情页的终端切换、输入框和附件可以立即使用。
  const added = !terminal().state.pending.some(x => x.name === info.name);
  if (added) terminal().state.pending.push({ ...info, started: composer().pendingStartedAt(info) });
  Search.cancelSearch(true);
  core.state.selection.sel = pendingUid(info.name);
  composer().rememberComposerSession(composer().composerDraft(core.state.selection.sel), info);
  core.state.selection.agent = null;
  core.preferences.set('sel', core.state.selection.sel);
  core.preferences.set('agent', null);
  // Clicking an existing pending row must reach term/claim immediately. A full
  // sidebar rebuild can take seconds for large histories and blocks attach.
  pendingStage().selectPendingSidebarRow(core.state.selection.sel, added);
  status().showSessionCount(core.index.sidebarSessions().length);
  const src = SOURCES[info.source];
  const pendingTitle = info.title || `新建 ${src.name} 会话`;
  document.querySelector('#detail').replaceChildren(sessionUi().head({uid:core.state.selection.sel,source:info.source,title:pendingTitle,node_name:info.node_name,cwd:info.cwd},0,info));
  const waiting=SessionUi.pendingStage(capabilities.config.backend==='rust'?terminal().pendingStageMessage(info):'');
  document.querySelector('#detail').appendChild(waiting);
  if (typeof core.diagnostics.auditDetailRendered === 'function') core.diagnostics.auditDetailRendered('new-session', {name: info.name});
  Shell.showMobileDetail();
  terminal().state.uid = core.state.selection.sel;
  composer().renderComposer();
  takeover().renderTakeoverBtn();
}

function renderPendingSessionAction(info, button = document.querySelector('#a-session-action')) {
  if (!button || core.state.selection.sel !== pendingUid(info.name)) return;
  // 行已不在列表里（别的页面删了、或已归档）：按已结束处理，绝不按旧 receipt 显示"停止"。
  const current = terminal().pendingSessionRow(info.name) || { ...info, running: false, stale: true, state: 'exited' };
  const stop = terminal().pendingShellRunning(current);
  const label = stop ? '停止会话' : '删除会话';
  SessionUi.pendingAction({label,icon:stop?'power':'trash',run:(target,pending)=>stop?terminal().stopPendingSession(info,target,pending):terminal().deletePendingSession(info,target,pending)});

}

function renderQueuedSends(uid = core.state.selection.sel) {
  const box = document.querySelector('#msgs'), stage = box ? null : document.querySelector('#detail .new-session-wait');
  const draft = uid && uid === core.state.selection.sel && !core.state.selection.agent ? composer().composerDrafts.get(composer().composerDraftOwner(uid)) : null;
  const rows = Array.isArray(draft?.cli?.queued) ? draft.cli.queued : [];
  Conversation.stageQueue(stage,uid,rows);
  if (box) Conversation.tail(box,{queued:rows,returned:draft?.cli?.input?.code === 'cli_input_returned'});
}
function selectPendingSidebarRow(uid,added){if(added)sidebarView().renderSide();else sidebarView().paintSidebarSelection(uid)}
return {showNewSessionStage,renderPendingSessionAction,renderQueuedSends,selectPendingSidebarRow}
}
