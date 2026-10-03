import {reactive,markRaw,nextTick} from 'vue'
import {state as searchState} from '../../stores/search'
import {STOP_STAGE_TEXT} from '../../domain/session-ui/stop-status'
/** @param {import('../../domain/session-ui/types').SessionUiPresentation} ui */
export function createHeaderController(bridge,ui){
function newAssignedLaunchFor(session) {
  if (typeof bridge.T === 'undefined' || !session?.sid) return null;
  return (bridge.T.pending || []).find(row => row.launch_kind === 'new_assigned'
    && row.source === session.source
    && String(row.declared_sid || '') === String(session.sid)) || null;
}

function unusedNewAssignedLaunch(session) {
  const launch = newAssignedLaunchFor(session);
  if (!launch) return null;
  // A catalog/header snapshot can predate the accepted conversation window.
  // Its zero cursor cannot make an already populated launch unused again.
  const accepted = bridge.cache.get(bridge.viewKey(session.uid, null));
  if (Number(session.cursor?.end) > 0 || Number(accepted?.end) > 0) return null;
  return launch;
}

function renderSessionFreeze(m, button = bridge.$('#a-session-freeze')) {
  if (!button || m.uid !== bridge.S.sel) return;
  const row = typeof bridge.T !== 'undefined' && (bridge.T.list || []).find(row => row.uid === m.uid);
  const reason = typeof bridge.T === 'undefined' || !bridge.T.listLoaded
    ? '正在读取会话运行状态，请稍后重试。'
    : !bridge.HUB_MODE && bridge.SessionDockCapabilities.config.session_freeze !== true
      ? '当前节点不支持冻结现场；此功能仅适用于已启用会话管理的 Linux 节点。'
      : row?.stale ? '会话运行状态已失效，请等待所属节点恢复连接。'
      : !row?.instance_id ? '当前会话没有可验证的运行实例，无法冻结现场。'
      : typeof row.frozen !== 'boolean'
        ? '当前节点不支持冻结现场；此功能仅适用于已启用会话管理的 Linux 节点。' : '';
  const frozen = !reason && row.frozen;
  const label = frozen ? '恢复运行' : '冻结现场';
  Object.assign(ui.header.freeze,{hidden:false,icon:frozen?'play':'pause',label,pressed:String(!!frozen),reason});
  if (reason) return;
  ui.header.freeze.run = async button => {

    ui.header.freeze.disabled = true;
    try {
      const result = await bridge.post('api/session/freeze', {uid:m.uid,
        instance_id:row.instance_id, frozen:!row.frozen});
      if (result.error || !result.ok) throw new Error(result.error || '请求失败');
      row.frozen = result.frozen;
      bridge.paintTurn(m.uid);
      bridge.browserAuditEvent('session.freeze', {uid:m.uid, instance_id:row.instance_id,
        frozen:result.frozen, process_count:result.process_count});
      syncSessionFreezeOverlay();
    } catch (error) {
      showSessionStopNotice(`冻结 / 恢复失败：${error.message || error}`, true, m.uid);
    } finally {
      await bridge.loadTermList();
      bridge.paintTurn(m.uid);
      renderSessionFreeze(m);
      if(ui.header?.meta.uid===m.uid)ui.header.freeze.disabled = false;
    }
  };
}

function renderSessionAction(m, button = bridge.$('#a-session-action')) {
  renderSessionFreeze(m,button?.closest('.dhead')?.querySelector('#a-session-freeze'));
  if(!button || m.uid!==bridge.S.sel || !ui.header)return;
  const action=ui.header.action;
  if(m.fork_parent){const shown=!!m.fork_parent_visible;Object.assign(action,{icon:'eye-off',label:shown?'隐藏父会话':'显示父会话',run:()=>setHeaderParentVisibility(m.uid,!shown)});return;}
  const launch=unusedNewAssignedLaunch(m);
  if(launch && typeof bridge.deletePendingSession==='function'){Object.assign(action,{icon:'trash',label:'删除会话',run:()=>{const h=ui.header;return bridge.deletePendingSession(launch,null,pending=>{h.action.disabled=pending;});}});return;}
  const running=sessionStoppable(m.uid);
  Object.assign(action,{icon:running?'power':'trash',label:running?'停止会话':'删除会话',run:button=>running?stopSession(m,button):del(m)});
}

// Rust `session_stop`: the server stops only a managed host instance (Ctrl-D,
// then the host's guarded stop); an unmanaged/external CLI is a typed refusal.
// Without process detection `S.live` only holds sessions this page launched or
// took over, so a listed managed instance also makes the session stoppable.
const sessionStopCapable = () => bridge.SessionDockCapabilities.config.backend === 'rust'
  && bridge.SessionDockCapabilities.config.session_stop === true;
function sessionStoppable(uid) {
  if (bridge.S.live.has(uid)) return true;
  return sessionStopCapable() && typeof bridge.T !== 'undefined'
    && (bridge.T.list || []).some(row => row.uid === uid && !!row.instance_id && !row.stale);
}
let sessionStopNoticeTimer = 0;
// Keep the frozen scene inside the selected session pane, never in global floats.
function syncSessionFreezeOverlay(){ui.frozen=bridge.sessionFrozen(bridge.S.sel)&&(!bridge.MOBILE.matches||document.body.classList.contains('mobile-detail'));ui.freezeUid=ui.frozen?bridge.S.sel:'';}
function syncSessionStopNotice(){syncSessionFreezeOverlay();if(ui.stopUid && (ui.stopUid!==bridge.S.sel || bridge.MOBILE.matches&&!document.body.classList.contains('mobile-detail'))){clearTimeout(sessionStopNoticeTimer);ui.stopHidden=true;}}
function showSessionStopNotice(text,sticky=false,uid=''){
 if(text&&(bridge.sessionStopBusy || bridge.S.picking && bridge.sessionStopProgress))return;
 clearTimeout(sessionStopNoticeTimer);ui.stop=text;ui.stopUid=uid;ui.stopHidden=!(text&&(!uid||uid===bridge.S.sel));
 if(text&&!sticky)sessionStopNoticeTimer=setTimeout(()=>{ui.stopHidden=true;},8000);
}

async function requestSessionStop(m) {
  const body = { uid: m.uid };
  if (sessionStopCapable()) body.request_id = globalThis.crypto?.randomUUID?.()
    || `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  const response = await fetch(bridge.appUrl('api/session/stop'), {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  const result = await response.json();
  if (!response.ok || result.error) throw new Error(result.error || response.status);
  if (sessionStopCapable() && result.stopped) bridge.S.live.delete(m.uid);
  return result;
}

async function stopSession(m, button = null) {
  // 空闲会话停止不打断任何工作，不再确认；轮转中、等回答或状态未知仍确认。
  if (bridge.sessionTurn(m.uid) !== 'idle' && !await bridge.appConfirm(`停止会话「${m.title}」?\n\n停止后才可以删除会话记录。`)) return;
  const owned = button?.id==='a-session-action' && button.closest('.dhead');
  if(owned)ui.header.action.disabled=true;else if (button) button.disabled = true;
  try {
    const d = await requestSessionStop(m);
    if (sessionStopCapable()) {
      showSessionStopNotice(`「${m.title}」${STOP_STAGE_TEXT[d.stage] || d.explanation || '停止请求已处理'}`);
    }
    await bridge.refreshLive(true);
    if (typeof bridge.loadTermList === 'function') await bridge.loadTermList();
    bridge.paintLive();
  } catch (error) {
    if (sessionStopCapable()) showSessionStopNotice(`停止失败：${error.message || error}`, true);
    else await bridge.appAlert(`停止失败：${error.message || error}`);
  } finally {
    if(owned){if(ui.header?.meta.uid===m.uid)ui.header.action.disabled=false;}else if (button) button.disabled = false;
  }
}

// Rust 回收站能力：文件进服务端显式配置的回收站目录。
const trashCapable = () => bridge.SessionDockCapabilities.config.backend === 'rust'
  && bridge.SessionDockCapabilities.config.trash === true;
const trashLocationNote = () => '文件会移入服务端回收站，不会永久删除。';
// 运行状态未知不等于已退出：只有用户明确确认 CLI 已退出，才带 force 重试。
function confirmForceDelete(count, detail) {
  return bridge.appConfirm(`${count === 1 ? '该会话' : `${count} 个会话`}的运行状态未知${detail ? `（${detail}）` : ''}，`
    + '后端无法确认 CLI 已经退出；未知不代表已停止。\n\n'
    + '请先确认这些会话的 CLI 都已退出。仍要删除吗?');
}

async function requestSessionDelete(uid, force = false) {
  const response = await fetch(bridge.appUrl('api/session/' + encodeURIComponent(uid) + (force ? '?force=1' : '')),
    { method: 'DELETE' });
  return { response, data: await response.json().catch(() => ({})) };
}

// OpenCode 会话在它自己的数据库里：直接删除（连同子会话），不进回收站。
const opencodeDeleteNote = () => 'OpenCode 会话会从 OpenCode 直接删除（连同子会话），不进回收站，无法恢复。';

async function del(m) {
  const opencode = m.source === 'opencode';
  if (!await bridge.appConfirm(`删除会话「${m.title}」?\n\n${opencode ? opencodeDeleteNote() : trashLocationNote()}`)) return;
  bridge.closeWatch();                         // 先停 SSE，避免文件移走后 EventSource 自动重连 404
  let { response, data } = await requestSessionDelete(m.uid);
  if (!response.ok && trashCapable() && data.code === 'run_state_unknown' && data.needs_force
      && await confirmForceDelete(1, data.run_state?.detail)) {
    ({ response, data } = await requestSessionDelete(m.uid, true));
  }
  if (!response.ok) {
    bridge.watchSession(m.uid);                // 删除失败，会话仍在，恢复实时同步
    return bridge.appAlert('删除失败: ' + (data.error || response.status));
  }
  bridge.forgetDeletedReceipts([m]);
  bridge.S.sessions = bridge.S.sessions.filter(x => x.uid !== m.uid);
  if (bridge.S.results) bridge.S.results = bridge.S.results.filter(x => x.uid !== m.uid);
  bridge.S.sel = null;
  bridge.store.set('sel', null);
  bridge.renderChips(); bridge.renderSide();
  bridge.mountDeleted({opencode,trash:data.trash});
  bridge.ensureConsolePlaceholder();
  bridge.auditDetailRendered('trashed');
  bridge.showMobileList();
}


let layoutSequence=0;
function head(m,total,pending=null){
 const h=reactive({meta:markRaw(m),pending:pending&&markRaw(pending),title:m.title,total,hasAgents:(m.agent_items||[]).length>0,icon:bridge.sessionIconMarkup(m.source,pending?true:bridge.S.live.has(m.uid),pending?true:bridge.S.liveTmux.has(m.uid),pending?'':bridge.sessionTurn(m.uid)),menuOpen:false,viewsOpen:false,chainOpen:false,views:[],chain:[],chainPending:[],inline:0,brief:0,moreHidden:false,moreLabel:'更多会话操作',items:[],metadata:[],freeze:reactive({id:'a-session-freeze',class:'session-menu-action',hidden:true,icon:'pause',label:'冻结现场',reason:'',run:null}),action:reactive({id:'a-session-action',class:'session-menu-action danger',icon:'trash',label:'删除会话',run:null})});
 ui.header=h;
 h.items=['new-session','settings','page-reload','transfer-tasks'].map(id=>reactive({id:'a-global-'+id,available:false,class:'session-menu-action',label:''}));
 const spec=(id,label,icon,cls='session-menu-action',extra={})=>reactive({id,label,icon,class:cls,...extra});
 if(!pending){h.items.push(spec('a-star',m.starred?'取消星标':'标为星标',m.starred?'star-filled':'star','star-toggle session-menu-action'+(m.starred?' on':''),{starUid:m.uid,pressed:String(!!m.starred),disabled:bridge.S.starBusy.has(m.uid)}));h.items.push(spec('a-turns',bridge.S.compactTurns?'展开所有过程':'折叠已完成过程','process','session-menu-action turn-mode'+(bridge.S.compactTurns?'':' on'),{pressed:String(!bridge.S.compactTurns)}));if(bridge.S.term){searchState.navigation={text:'…',capped:false,title:undefined};h.items.push({kind:'search'});}}
 h.items.push({kind:'diagnostics'});
 if(!pending&&bridge.SessionDockCapabilities.config.session_clone_local_codex===true)h.items.push(spec('a-clone-group','移动 / 复制整组','transfer'));
 if(!m.agent_id)h.items.push({kind:'action'});
 h.metadata=[{id:'mcount-total',text:`${total} 条消息`}];
 if(!pending){h.metadata.push({class:'meta-secondary',text:bridge.fmtSize(m.size)},{class:'meta-secondary',text:`${bridge.fmtTime(m.created)} → ${bridge.fmtTime(m.updated)}`});}
 if(m.node_name)h.metadata.push({class:'meta-node node-badge',color:bridge.nodeColor(m.node_name),text:m.node_name});
 h.metadata.push({class:'meta-secondary',code:true,text:bridge.shortCwd(m.cwd||(pending?'':'(未知)'),999)},{class:'meta-source',text:m.agent_type||bridge.SOURCES[m.source].name});
 if(!pending){if(m.model)h.metadata.push({class:'meta-secondary',text:m.model});h.metadata.push({class:'meta-secondary session-id',code:true,text:m.sid});}
 return bridge.mountHeader();
}
function closeSessionActions(restoreFocus=false){if(!ui.header?.menuOpen)return;ui.header.menuOpen=false;if(restoreFocus)bridge.$('#a-more')?.focus();}
function closeForkChainMenu(){if(ui.header)ui.header.chainOpen=false;}
function sessionViewRows(m){
 const row=bridge.S.sessions.find(s=>s.uid===m.uid),items=[...((row||m).agent_items||[])];const running=a=>!!a.active&&bridge.S.live.has(m.uid);const when=value=>Date.parse(value||'')||0;
 items.sort((a,b)=>(running(b)-running(a))||(when(b.updated)-when(a.updated)));
 return [{id:'',title:m.parent_title||m.title,on:!m.agent_id,main:true},...items.map(a=>({id:a.id,title:a.title,on:m.agent_id===a.id,running:running(a),type:a.type,span:bridge.fmtSpan(a.created,running(a)?null:a.updated)}))];
}
function toggleViews(e){e.stopPropagation();const h=ui.header;if(!h.viewsOpen)h.views=sessionViewRows(h.meta);h.viewsOpen=!h.viewsOpen;if(h.viewsOpen)setTimeout(()=>document.addEventListener('click',()=>{if(ui.header===h)h.viewsOpen=false;},{once:true}),0);}
function selectView(agent,e){e.stopPropagation();ui.header.viewsOpen=false;bridge.openSession(ui.header.meta.uid,agent||null);}
function renderForkChainMenu(){const h=ui.header;if(!h?.chainOpen)return;const current=bridge.S.sessions.find(s=>s.uid===bridge.S.sel)||(bridge.S.results||[]).find(s=>s.uid===bridge.S.sel)||h.meta;const chain=current?bridge.forkAncestors(current):[],children=current?bridge.forkChildren(current):[];h.chain=chain.map(({sid,row},i)=>({sid,row:row&&markRaw(row),level:i===0?'父会话':`上 ${i+1} 级父会话`})).concat(children.map((row,i)=>({row:markRaw(row),level:children.length>1?`子会话 ${i+1}`:'子会话'})));}
function toggleChain(e){e.stopPropagation();const h=ui.header;if(h.chainOpen){closeForkChainMenu();return;}closeSessionActions();h.viewsOpen=false;h.chainOpen=true;renderForkChainMenu();}
async function setHeaderParentVisibility(uid,visible){const h=ui.header;h.action.disabled=true;try{return await bridge.setForkParentVisibility([uid],visible,null);}finally{h.action.disabled=false;}}
async function chainVisibility(row){const visible=!row.fork_parent_visible,h=ui.header;h.chainPending.push(row.uid);try{await bridge.setForkParentVisibility([row.uid],visible,null);}finally{h.chainPending=h.chainPending.filter(uid=>uid!==row.uid);}if(visible)bridge.$(`#side .item[data-uid="${CSS.escape(row.uid)}"]`)?.scrollIntoView({block:'nearest'});}
function chainOpen(row){closeForkChainMenu();bridge.openSession(row.uid,null,{exact:true});}
function toggleActions(){const h=ui.header;if(h.menuOpen){closeSessionActions();return;}h.menuOpen=true;h.viewsOpen=false;closeForkChainMenu();}
function actionKey(e){if(!['ArrowDown','ArrowUp'].includes(e.key))return;e.preventDefault();if(!ui.header.menuOpen)toggleActions();nextTick(()=>{const rows=[...bridge.$('#session-actions-menu').querySelectorAll('button:not(:disabled)')];(e.key==='ArrowUp'?rows.at(-1):rows[0])?.focus();});}
function menuKey(e){const rows=[...bridge.$('#session-actions-menu').querySelectorAll('button:not(:disabled)')],index=rows.indexOf(document.activeElement);const next={ArrowDown:(index+1)%rows.length,ArrowUp:(index-1+rows.length)%rows.length,Home:0,End:rows.length-1}[e.key];if(next===undefined)return;e.preventDefault();rows[next]?.focus();}
function focusout(e){if(e.relatedTarget&&!bridge.$('#a-more').parentElement?.contains(e.relatedTarget))closeSessionActions();}
function toggleTurns(){bridge.S.compactTurns=!bridge.S.compactTurns;bridge.store.set('compactTurns',bridge.S.compactTurns);const spec=ui.header.items.find(s=>s.id==='a-turns');Object.assign(spec,{class:'session-menu-action turn-mode'+(bridge.S.compactTurns?'':' on'),pressed:String(!bridge.S.compactTurns),label:bridge.S.compactTurns?'展开所有过程':'折叠已完成过程'});document.querySelectorAll('#msgs > .turn-process').forEach(node=>bridge.S.compactTurns?node._fold?.():node._open?.());bridge.refreshMessageTimeDividers();bridge.settle(bridge.$('#msgs'));}
function toggleStar(){return bridge.toggleSessionStar(ui.header.meta.uid);}
function cloneGroup(){return bridge.cloneSessionGroup(ui.header.meta.uid);}
function freezeSession(button){return ui.header.freeze.run?.(button);}
function sessionAction(button){const h=ui.header;return h.pending ? h.action.run?.(null,pending=>{h.action.disabled=pending;}) : h.action.run?.(button);}
function setActionPending(pending){if(ui.header)ui.header.action.disabled=pending;}
function newSessionProxy(){document.getElementById('new-session')?.click();}
function settingsProxy(){document.getElementById('settings')?.click();}
function reloadProxy(){document.getElementById('page-reload')?.click();}
function transfersProxy(){document.getElementById('transfer-tasks')?.click();}
function paintStarButton(button,starred,busy=false){if(!button)return;const spec=ui.header?.items.find(s=>s.id==='a-star');if(spec)Object.assign(spec,{class:'star-toggle session-menu-action'+(starred?' on':''),label:starred?'取消星标':'标为星标',icon:starred?'star-filled':'star',pressed:String(starred),disabled:busy});}
function syncSessionGlobalActions(heading,list){
 if(heading&&!heading.isConnected)return;
 const dock=!!list&&(document.body.classList.contains('side-collapsed')||bridge.MOBILE.matches&&document.body.classList.contains('mobile-detail'));
 let changed=false;const h=ui.header;if(!h)return;

 for(const [index,id] of ['new-session','settings','page-reload','transfer-tasks'].entries()){
  const source=document.getElementById(id);if(!source)continue;const action=bridge.SessionDockShell.actionState(id),enabled=dock&&!action.hidden&&!action.capabilityHidden;
  const proxy=h.items.find(item=>item.id==='a-global-'+id);
  proxy.available=enabled;
  if(enabled)Object.assign(proxy,{iconHtml:source.querySelector('svg').outerHTML,label:source.ariaLabel||source.title,disabled:source.disabled,order:index-4});
  if(bridge.SessionDockShell.dockAction(id,enabled))changed=true;
 }
 if(changed)bridge.layoutHeader();
}
async function layoutSessionHead(heading=bridge.$('#detail .dhead')){
 const h=ui.header,wrap=heading?.querySelector('.session-actions'),menu=heading?.querySelector('#session-actions-menu'),list=menu?.querySelector('[role="menu"]');
 syncSessionGlobalActions(heading,list);if(!h||!wrap||!menu||!list)return;
 const sequence=++layoutSequence;closeSessionActions();h.inline=0;h.brief=0;h.moreHidden=false;
 await nextTick();if(sequence!==layoutSequence||ui.header!==h||!heading.isConnected)return;
 const actions=wrap.parentElement,tier=bridge.layoutTier(),title=heading.querySelector('.dtitle'),h2=title.querySelector('h2');
 const gap=parseFloat(getComputedStyle(title).columnGap)||0;
 const titleMax=tier==='narrow'?Infinity:Math.max(8*parseFloat(getComputedStyle(h2).fontSize),title.clientWidth*0.4);
 const titleWidth=Math.min(h2.getBoundingClientRect().width,titleMax);
 const room=actions.getBoundingClientRect().left-h2.getBoundingClientRect().left-titleWidth-gap*2-24;
 let used=0;const actionsGap=parseFloat(getComputedStyle(actions).columnGap)||0;
 for(let i=0;i<h.items.length;i++){
  h.inline=i+1;await nextTick();if(sequence!==layoutSequence||ui.header!==h)return;
  const node=actions.querySelector(`[data-order="${i}"]`);if(!node)continue;const width=node.getBoundingClientRect().width+actionsGap;
  if(used+width>room){h.inline=i;await nextTick();break;}used+=width;
 }
 if(h.inline===h.items.length){
  h.brief=h.metadata.length;await nextTick();const brief=heading.querySelector('.dbrief'),briefGap=parseFloat(getComputedStyle(brief).columnGap)||0;let keep=0;
  for(const item of brief.children){const width=item.getBoundingClientRect().width+(keep?briefGap:0);if(used+width>room)break;used+=width;keep++;}h.brief=keep;
 }
 heading.classList.toggle('head-flat',h.inline===h.items.length);
 h.moreLabel=h.inline===h.items.length?'会话信息':'更多会话操作';h.moreHidden=h.inline===h.items.length&&h.brief===h.metadata.length;
 await nextTick();if(sequence===layoutSequence)bridge.auditHeaderLayout(heading,tier,actions);
}
async function headerMounted(){const h=ui.header;if(!h)return;bridge.showConsoleToast('');if(h.pending)bridge.renderPendingSessionAction(h.pending);else renderSessionAction(h.meta);bridge.paintTransferAvailability(bridge.$('#a-clone-group'),h.meta.uid);if(typeof bridge.renderTakeoverBtn==='function')setTimeout(bridge.renderTakeoverBtn,0);syncSessionStopNotice();layoutSessionHead();}
function mobileBack(){bridge.showMobileList();}
function consoleHint(){const m=ui.header?.meta || {uid:bridge.S.sel,agent_id:bridge.S.agent};bridge.showConsoleToast(bridge.consoleUnavailableReason(m.uid,m.agent_id));}
function clearConsoleHint(){bridge.showConsoleToast('');}
async function consoleClick(button){
 const m=ui.header?.meta || {uid:bridge.S.sel,agent_id:bridge.S.agent},uid=m.uid,agent=m.agent_id;bridge.showConsoleToast('');const reason=bridge.consoleUnavailableReason(uid,agent,false);if(bridge.ConsoleUI.busy.has(uid))return bridge.showConsoleToast(reason);if(reason)return bridge.appAlert('控制台不可用：\n'+reason);
 bridge.ConsoleUI.busy.add(uid);bridge.ConsoleUI.errors.delete(uid);bridge.paintConsoleAvailability(button,uid,agent);
 try{if(bridge.linkedTermSession(uid,{followReplacement:true}))await bridge.toggleLinkedTermSession(uid);else await bridge.takeover(uid,button);}
 catch(error){const message=error.message||String(error);bridge.ConsoleUI.errors.set(uid,message);await bridge.appAlert('打开控制台失败：\n'+message);}
 finally{bridge.ConsoleUI.busy.delete(uid);if(typeof bridge.renderTakeoverBtn==='function')bridge.renderTakeoverBtn();if(bridge.ConsoleUI.errors.has(uid))bridge.showConsoleToast(bridge.ConsoleUI.errors.get(uid));}
}
function initialize(){
 document.addEventListener('click',e=>{if(!e.target.closest('.session-actions'))closeSessionActions();if(!e.target.closest('#fork-chain-menu,#a-fork-chain'))closeForkChainMenu();},true);
 document.addEventListener('keydown',e=>{if(e.key!=='Escape')return;if(ui.header?.menuOpen){e.preventDefault();e.stopImmediatePropagation();closeSessionActions(true);}else if(ui.header?.chainOpen){e.preventDefault();e.stopImmediatePropagation();closeForkChainMenu();bridge.$('#a-fork-chain')?.focus();}},true);
 addEventListener('resize',()=>closeSessionActions());for(const media of [bridge.MOBILE,bridge.MEDIUM])media.addEventListener('change',()=>layoutSessionHead());
 let detailWidth=-1;new ResizeObserver(entries=>{const width=entries.at(-1)?.contentRect.width??-1;if(width===detailWidth)return;detailWidth=width;layoutSessionHead();}).observe(bridge.$('#detail'));
 new MutationObserver(()=>layoutSessionHead()).observe(bridge.$('#detail'),{childList:true});document.fonts?.ready.then(()=>layoutSessionHead());
}
return {setActionPending,initialize,head,headerMounted,paintStarButton,closeSessionActions,closeForkChainMenu,renderForkChainMenu,layoutSessionHead,syncSessionGlobalActions,sessionViewRows,renderSessionAction,renderSessionFreeze,newAssignedLaunchFor,unusedNewAssignedLaunch,sessionStopCapable,sessionStoppable,syncSessionFreezeOverlay,syncSessionStopNotice,showSessionStopNotice,requestSessionStop,stopSession,trashCapable,trashLocationNote,confirmForceDelete,requestSessionDelete,opencodeDeleteNote,del,mobileBack,toggleViews,selectView,toggleChain,chainVisibility,chainOpen,toggleActions,actionKey,menuKey,focusout,toggleStar,toggleTurns,cloneGroup,freezeSession,sessionAction,newSessionProxy,settingsProxy,reloadProxy,transfersProxy,consoleHint,clearConsoleHint,consoleClick,get forkLabel(){const m=ui.header?.meta;return m&&!m.agent_id&&(m.forked_from_id||m.fork_parent||bridge.forkChildren(m).length)?m.forked_from_id?'父会话链':'子会话':'';},formatTime:bridge.fmtTime};
}
