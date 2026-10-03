import * as SessionUi from '../../migration/session-ui'
const $ = selector => document.querySelector(selector);

import * as Search from '../../migration/search'

import * as Conversation from '../../migration/conversation'

import * as Composer from '../../migration/composer'

import {sessiondockCli} from '../../domain/runtime/cli.js'

import {viewKey,entryTotal} from '../../domain/runtime/messages.js'
import {fmtSize} from '../../domain/runtime/format.js'

export function createConversationRenderer({core,sessionUi,detailNotices,syntaxRuntime,composer,terminal,questionRuntime,pendingStage,presentation}) {
function progress(done,total,label,detail={}) {const bar=presentation.progress;bar.active=true;bar.width=(total?Math.min(100,done/total*100):12)+'%';bar.idle=!total;const estimate=detail.estimated?'≈':'',compressed=detail.compressed?'（压缩）':'';bar.text=total?`${label} ${label==='渲染'?`${done}/${total}`:estimate+fmtSize(done)+' / '+fmtSize(total)+compressed}`:`${label}…`;}
function progressDone(){presentation.progress.active=false;presentation.progress.width='0%';}
function ensureConsolePlaceholder(){if($('#a-term'))return;$('#detail').prepend(SessionUi.consolePlaceholder());SessionUi.consoleToast('');}
let renderSeq=0;
async function renderSession(meta, msgs, activity = null, {startWatch = true, historyPageEntry = null} = {}) {
  const seq = ++renderSeq, uid = meta.uid, agent = meta.agent_id || null;
  if (core.state.selection.sel !== uid || core.state.selection.agent !== agent) return;
  const renderedLength = msgs.length, d = $('#detail');
  core.scroll.disposeMessageObservers($('#msgs'));
  d.replaceChildren();
  const entry = core.cache.cache.get(viewKey(uid, agent));
  // Mount the existing Vue header before preparing the conversation.
  Conversation.mountHeader(d,meta,entryTotal(entry || {msgs}));
  sessionUi().layoutSessionHead(); detailNotices().renderTimelinePinNotice(meta);
  const box = Conversation.mount(d);
  core.diagnostics.auditDetailRendered('render', {messages:msgs.length,seq});
  core.state.search.cur = -1; core.state.search.autoOpen = 0; core.state.search.markCapped = false;
  Conversation.planning.configurePlanning(core.state.sidebar.compactTurns);
  const partial = entry?.partial, split = partial ? Math.min(+partial.head || 0, msgs.length) : 0;
  const openTail = activity?.state === 'working';
  const tailComplete = (activity && !['working','waiting'].includes(activity.state)) || (!activity && !core.state.live.live.has(uid));
  const plan = partial ? [...Conversation.planning.planTurns(msgs.slice(0,split),{tailComplete:false,foldTail:true}),
    {gap:{uid,agent,omitted:partial.omitted}}, ...Conversation.planning.planTurns(msgs.slice(split),{openTail,tailComplete})]
    : Conversation.planning.planTurns(msgs,{openTail,tailComplete});
  // Preserve the renderer's cancellable batch yields before publication.
  for (let i = 0; i < plan.length; i += 250) {
    Conversation.prepare(box,plan.slice(i,i + 250),i === 0,{uid,agent});
    if (i + 250 < plan.length) {
      progress(i + 250,plan.length,'渲染');
      await new Promise(r => setTimeout(r,0));
      if (seq !== renderSeq || core.state.selection.sel !== uid || core.state.selection.agent !== agent) return;
    }
  }
  Conversation.publishPrepared(box);
  const latest = core.cache.cache.get(viewKey(uid,agent));
  // The authoritative cache may grow while either page or reset render yields.
  if (latest && (!historyPageEntry || latest === historyPageEntry)) {
    if (latest.msgs !== msgs || latest.msgs.length !== renderedLength)
      Conversation.append(box,latest.msgs.slice(renderedLength),null,{openTail:latest.activity?.state === 'working'});
    activity = latest.activity;
    if (activity?.state !== 'working') sealTurnTail(box,latest,{defer:false});
  }
  syntaxRuntime().scheduleSyntax(); renderConversationTail(activity,uid);
  core.scroll.stickBottom(box,true); core.scroll.watchBottom(box); progressDone();
  Search.markMatches(box); Search.updateMatchNav({jump:true});
  if (typeof composer().renderComposer === 'function') {if (core.state.selection.agent) Composer.operations().hide();else composer().renderComposer();}
  if (seq === renderSeq && core.state.selection.sel === uid && core.state.selection.agent === agent) {
    core.sync.renderMigrationReadFailure(uid,agent);
    if (startWatch) core.sync.watchSession(uid,agent);
    if (typeof terminal().restoreTermPane === 'function') terminal().restoreTermPane(uid,agent);
    core.diagnostics.scheduleBrowserSnapshot('rendered');
  }
}

let headerLayoutSignature = '';

function auditHeaderLayout(heading, tier, actions) {
  if (!heading.isConnected) return;   // head() 构建时先量一次，挂上之后才是真实排版
  try {
    const title = heading.querySelector('.dtitle');
    const brief = heading.querySelector('.dbrief');
    const data = {
      tier,
      inline: [...actions.querySelectorAll('button')].filter(b => !b.hidden && b.offsetWidth)
        .map(b => b.id || (b.dataset.reportBug !== undefined ? 'report-bug' : b.className.split(' ')[0])),
      brief: brief && !brief.hidden ? brief.children.length : 0,
      menu_meta: heading.querySelector('#session-actions-menu .dmeta')?.children.length ?? null,
      title_overflow: title ? title.scrollWidth - title.clientWidth : null,
      width: title?.clientWidth ?? null, head_height: heading.offsetHeight,
    };
    const signature = JSON.stringify(data);
    if (signature === headerLayoutSignature) return;
    headerLayoutSignature = signature;
    core.audit.browserAuditEvent('header.layout', data);
  } catch { /* 审计不能影响排版 */ }
}

function refreshMessageTimeDividers(box = $('#msgs')) {
  if (box) Conversation.refresh(box);
}

function sealTurnTail(box, entry, options = {}) {
  Conversation.planning.configurePlanning(core.state.sidebar.compactTurns);
  return Conversation.seal(box, entry, options);
}

function flushPendingTurnSeal(box) {
  if (!box?._turnSealPending || box !== $('#msgs')) return;
  box._turnSealPending = false;
  const entry = core.cache.cache.get(viewKey(core.state.selection.sel, core.state.selection.agent));
  if (entry) renderSession(entry.meta, entry.msgs, entry.activity, {startWatch:false});
}

function renderActivity(activity) {
  const box = $('#msgs'); if (!box) return;
  let visible = activity && ['working','waiting','aborted','failed'].includes(activity.state) ? activity : null;
  if (visible?.state === 'working') {
    const processStart = core.state.live.liveStarted.get(core.state.selection.sel), at = Date.parse(visible.ts) / 1000;
    if (!core.state.live.live.has(core.state.selection.sel) || (Number.isFinite(processStart) && Number.isFinite(at) && at < processStart - 2)) visible = null;
  }
  Conversation.tail(box, {activity:visible});
}

function renderTerminalThreadNotice(uid = core.state.selection.sel) {
  const box = $('#msgs'); if (!box) return;
  Conversation.tail(box, {sideThread:!core.state.selection.agent && uid === core.state.selection.sel && sessiondockCli(uid)?.source === 'codex' && !!terminal().codexSideThreadVisible(uid)});
}

function renderConversationTail(activity, uid = core.state.selection.sel) {
  const box = $('#msgs'); if (!box) return;
  const entry = core.cache.cache.get(viewKey(uid)), prompt = entry?.prompt;
  const nativeQuestion = prompt?.questions?.length ? null : questionRuntime().pendingHistoryQuestion(entry);
  const activeQuestion = prompt?.questions?.length ? prompt : nativeQuestion;
  questionRuntime().pruneQuestionFormDrafts(uid, activeQuestion && (activeQuestion.state || 'waiting') === 'waiting' ? String(activeQuestion.id || activeQuestion.call_id || '') : '');
  const question = prompt?.questions?.length ? {
    role:'question', call_id:prompt.id, questions:prompt.questions,
    text:prompt.questions.map(q => q.question).join('\n\n'), live:true,
    state:prompt.state, uid, ts:prompt.ts || prompt.created_at,
  } : nativeQuestion ? {...nativeQuestion, live:true, uid} : null;
  Conversation.tail(box, {uid,agent:core.state.selection.agent,question,shadow:question?.call_id || '',activity:null});
  if (!question) renderActivity(activity);
  renderTerminalThreadNotice(uid);
  if (typeof composer().syncComposerSendState === 'function') composer().syncComposerSendState();
  if (typeof pendingStage().renderQueuedSends === 'function') pendingStage().renderQueuedSends(uid);
  core.diagnostics.scheduleBrowserSnapshot('conversation-tail');
}
return {cancel(){renderSeq++},progress,progressDone,ensureConsolePlaceholder,get renderSeq(){return renderSeq},renderSession,get headerLayoutSignature(){return headerLayoutSignature},auditHeaderLayout,refreshMessageTimeDividers,sealTurnTail,flushPendingTurnSeal,renderActivity,renderTerminalThreadNotice,renderConversationTail};
}
