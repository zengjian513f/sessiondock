import {reactive,markRaw,nextTick} from 'vue'
import {compareVersions} from '../../domain/client-versions'
/** @param {import('../../domain/session-ui/types').SessionUiPresentation} ui */
export function createTransferController(bridge,ui){
function transferUnavailableReason(uid) {
  return uid && bridge.sessionStoppable(uid) ? '会话正在运行，请先停止后再移动或复制整组。' : '';
}
function paintTransferAvailability(button, uid) {
  const row = button?.closest('#item-menu')
    ? bridge.sidebarSessions().find(session => session.uid === uid) : null;
  const unavailable = button?.closest('#item-menu') && (!row || row.pending
    || bridge.SessionDockCapabilities.config.session_clone_local_codex !== true);
  const reason=unavailable ? '此会话当前不支持移动或复制整组。' : transferUnavailableReason(uid);
  const spec=button?.closest('.dhead') && ui.header?.items.find(item=>item.id==='a-clone-group');
  if(spec)spec.reason=reason;
  if(!spec)bridge.setControlUnavailable(button,reason);
}
async function cloneSessionGroup(uid, resumed = null) {
  const reason = resumed ? '' : transferUnavailableReason(uid);
  if (reason) {
    const control = bridge.$('#item-menu:not([hidden]) [data-act="clone"]') || bridge.$('#a-clone-group');
    paintTransferAvailability(control, uid); return;
  }
  bridge.closeSessionActions();
  bridge.unmountTransfer();
  const sourceId = bridge.nodeOf(uid);
  const machines = new Map();
  for (const node of [...bridge.Nodes.machines, ...bridge.Nodes.list]) machines.set(node.id, {...machines.get(node.id), ...node});
  const sourceName = machines.get(sourceId)?.name || (bridge.HUB_MODE ? '来源机器' : '当前机器');
  if (!machines.has(sourceId)) machines.set(sourceId, {id:sourceId, name:sourceName});
  const view = reactive({members:[],options:[], actions:markRaw({}), sourceName, targetNode:sourceId, mode:'clone',newIds:true,identityVisible:false,notice:'',confirmLabel:'复制整组',confirmDisabled:true,controlsDisabled:false,abortVisible:false,abortLabel:'撤回本次移动',aborting:false,busy:false,status:'正在读取清单…',environmentVisible:false,environmentText:'',environmentTitle:'',progressVisible:false,progressText:'',progressPhase:'',errorVisible:false,errorText:''});
  ui.transfer = view;
  const dialog = await bridge.mountTransfer();
  const $d = selector => dialog.querySelector(selector);
  const target = $d('#transfer-target');
  view.options = [...machines.values()].map(node=>({value:node.id,label:node.name + (node.online===false || node.enabled===false?' · 不可用':''),disabled:(node.online===false || node.enabled===false) && node.id!==sourceId}));
  await nextTick();

  let plan = resumed?.plan || null, busy = false, planning = false, uncertain = !!resumed;
  let operationStarted = !!resumed, progressTimer = null, progressLoading = false, aborting = false, executionSequence = 0;
  let environmentLoading = false, environmentSequence = 0;
  const environmentClients = new Map();
  const identityChoices = {clone:true, move:false};
  if (resumed) {
    if (!machines.has(resumed.request.target_node)) {
      view.options.push({value:resumed.request.target_node,label:'目标机器不可用',disabled:true});
      await nextTick();
    }
    view.targetNode = resumed.request.target_node;
    view.mode = plan.mode;
    identityChoices[plan.mode] = plan.new_ids;
  }
  const mode = () => view.mode;
  const crossMachine = () => view.targetNode !== sourceId;
  const blockedReason = () => {
    const destination = machines.get(view.targetNode);
    if (!destination || destination.online === false || destination.enabled === false) return '目标机器当前不可用。';
    if (machines.get(sourceId)?.online === false) return '源机器已离线。';
    if (crossMachine()) {
      if (bridge.SessionDockCapabilities.config[mode() === 'move' ? 'session_move_remote' : 'session_clone_remote'] !== true) return '跨机器传输尚未接入。';
      return '';
    }
    if (mode() === 'move') return '移动需要选择另一台机器。';
    return '';
  };
  const renderSelection = () => {
    const cross = crossMachine(), moving = mode() === 'move';
    view.identityVisible = cross;
    view.newIds = identityChoices[mode()];
    const reason = blockedReason(); view.notice = reason;
    view.confirmLabel = aborting ? '正在撤回…' : busy && operationStarted ? (moving ? '正在移动…' : '正在复制…') : uncertain ? (moving ? '重试同一次移动' : '重试同一次复制') : moving ? '移动整组' : '复制整组';
    view.confirmDisabled = busy || planning || aborting || environmentLoading || !plan || !!reason;
    // Keep the chosen operation fixed while its publication result is uncertain.
    view.controlsDisabled = busy || aborting || uncertain;
    view.abortVisible = uncertain || (busy && operationStarted);
    view.abortLabel = moving ? '撤回本次移动' : '取消本次复制';
    view.aborting = aborting;
    view.busy = busy;
  };
  view.actions.targetChanged = event => {view.targetNode=event.target.value;renderSelection();refreshEnvironment();};
  view.actions.modeChanged = event => {view.mode=event.target.value;renderSelection();};
  view.actions.identityChanged = event => {identityChoices[mode()]=event.target.checked;renderSelection();};
  const close = () => {clearInterval(progressTimer); dialog.close(); ui.transfer=null; bridge.unmountTransfer(); refreshTransferTasks();};
  const cancelAndClose = () => cancelTransfer(true);
  view.actions.close=cancelAndClose;
  view.actions.cancel=e=>{e.preventDefault();cancelAndClose();};
  renderSelection(); await nextTick(); dialog.showModal(); target.focus();
  const request = async (path, body) => {
    const response = await fetch(bridge.appUrl(path), {method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify(body)});
    const data = await response.json();
    if (!response.ok) throw Object.assign(new Error(data.error?.message || data.error || '操作失败'), {code:data.code});
    return data;
  };
  async function discardPreview(previous) {
    if (previous) await request('api/session/clone/cancel', {uid, operation_id:previous.operation_id});
  }
  async function cancelTransfer(closing = false) {
    if (aborting) return;
    if (!plan) { if (closing) close(); return; }
    ++executionSequence; aborting = true; busy = true; view.errorVisible = false; renderSelection();
    try {
      const result = operationStarted && bridge.HUB_MODE
        ? await request('api/session/transfer/cancel', {uid, operation_id:plan.operation_id, target_node:view.targetNode})
        : await request('api/session/clone/cancel', {uid, operation_id:plan.operation_id});
      uncertain = false; operationStarted = false; plan = null; busy = false;
      view.progressVisible = false; refreshTransferTasks();
      if (closing || result.phase === 'complete') {close(); await bridge.loadSessions(true);}
      else await refreshPlan();
    } catch (failure) {
      uncertain = operationStarted;
      if (dialog.isConnected) {view.errorText = failure.message; view.errorVisible = true;}
    } finally {aborting = false; busy = false; if (dialog.isConnected) renderSelection();}
  }
  view.actions.abort = () => cancelTransfer();
  const renderMembers = data => {
    const members = new Map();
    for (const member of data.sessions) {
      const key = `${member.source}:${member.sid}`;
      if (!members.has(key)) members.set(key, {...member, files:0, bytes:0, selected:false, relations:new Set()});
      const row = members.get(key);
      row.files += member.file_count ?? 1; row.bytes += member.bytes ?? 0;
      row.selected ||= member.uid === uid;
      if (member.uid === uid) {row.title = member.title; row.cwd = member.cwd;}
      for (const relation of member.relations || []) row.relations.add(relation);
    }
    const ordered = [...members.values()].sort((a,b)=>Number(b.selected)-Number(a.selected)||Number(a.agent)-Number(b.agent));
    view.members = ordered.map(member=>({source:member.source,sid:member.sid,selected:member.selected,files:member.files, name:member.title || member.sid, detail:member.cwd || member.sid, detailTitle:`${member.sid}${member.cwd?'\n'+member.cwd:''}`, sourceName:{codex:'Codex',claude:'Claude',grok:'Grok'}[member.source]||member.source, badge:member.selected?'所选会话':member.agent?'子代理':member.relations.has('fork')?'分支关联':'关联历史', sizeLabel:bridge.fmtSize(member.bytes)}));
    view.status = `整组 ${data.session_count} 个会话 · ${data.file_count} 份历史 · ${bridge.fmtSize(data.bytes)}`;
  };
  async function refreshEnvironment() {
    const sequence = ++environmentSequence;
    if (!plan) {environmentLoading = false; view.environmentVisible = false; renderSelection(); return;}
    const destinationId = view.targetNode, currentPlan = plan;
    const clients = id => {
      if (!environmentClients.has(id)) environmentClients.set(id, (async () => {
        try {
          const path = bridge.HUB_MODE ? `api/nodes/${id}/api/clients` : 'api/clients';
          const response = await fetch(bridge.appUrl(path), {cache:'no-store', signal:AbortSignal.timeout(20000)});
          const data = await response.json();
          if (!response.ok || !Array.isArray(data.clients)) throw new Error('clients unavailable');
          return data.clients;
        } catch {return null;}
      })());
      return environmentClients.get(id);
    };
    environmentLoading = true; view.environmentVisible = true; view.environmentText = '正在核对目标 CLI…'; view.environmentTitle = '';
    renderSelection();
    const [sourceClients, targetClients] = await Promise.all([clients(sourceId), clients(destinationId)]);
    if (!dialog.isConnected || sequence !== environmentSequence) return;
    const messages = [], cross = destinationId !== sourceId;
    if (!targetClients) messages.push('未核验目标 CLI');
    if (cross && !sourceClients) messages.push('未核验源 CLI 版本');
    for (const provider of new Set(currentPlan.sessions.map(member => member.source))) {
      const name = bridge.SOURCES[provider]?.name || provider;
      const installed = (targetClients || []).filter(client => client.source === provider && client.installed);
      if (targetClients && !installed.length) {messages.push(`目标未配置可用的 ${name} CLI`); continue;}
      if (!cross || !targetClients || !sourceClients) continue;
      const versions = rows => rows.map(client => client.version).filter(version => typeof version === 'string' && /^\d+(?:\.\d+)+/.test(version));
      const targetVersions = versions(installed);
      const sourceVersions = versions(sourceClients.filter(client => client.source === provider && client.installed));
      if (!targetVersions.length || !sourceVersions.length) messages.push(`未核验 ${name} 版本差异`);
      else if (targetVersions.some(to => sourceVersions.some(from => compareVersions(to, from) < 0))) messages.push(`目标 ${name} 存在较旧版本`);
    }
    const tools = Array.isArray(currentPlan.dynamic_tools) ? currentPlan.dynamic_tools : [];
    if (!Array.isArray(currentPlan.dynamic_tools)) messages.push('未核验动态工具依赖');
    if (tools.length) messages.push(`${tools.length} 个动态工具执行器未核验`);
    view.environmentText = messages.join('；'); view.environmentTitle = tools.join('、'); view.environmentVisible = !!messages.length;
    environmentLoading = false; renderSelection();
  }
  async function refreshPlan(executing = false) {
    if ((!executing && busy) || planning || uncertain || (mode() === 'move' && !crossMachine())) return;
    const fresh = !crossMachine() || identityChoices[mode()];
    const selectedMode = mode();
    if (plan && plan.new_ids === fresh && plan.mode === selectedMode) return true;
    const previous = plan;
    planning = true; plan = null; view.errorVisible = false; renderSelection();
    view.status = '正在读取清单…';
    try {
      const next = await request('api/session/clone/plan', {uid, new_ids:fresh, mode:selectedMode});
      if (!fresh && next.new_ids !== false) throw new Error('源机器版本尚不支持保留 UID，请更新节点');
      if (selectedMode === 'move' && next.mode !== 'move') throw new Error('源机器版本尚不支持移动，请更新节点');
      if (previous && previous.operation_id !== next.operation_id) await discardPreview(previous);
      if (!dialog.isConnected) {await discardPreview(next); return false;}
      plan = next;
      if (dialog.isConnected) {renderMembers(plan); refreshEnvironment();}
      return true;
    } catch (failure) {
      plan = previous;
      if (dialog.isConnected) {
        view.status = '清单读取失败';
        view.errorText = failure.message; view.errorVisible = true;
      }
      return false;
    } finally {planning = false; if (dialog.isConnected) renderSelection();}
  }
  const paintProgress = data => {
    view.progressVisible = true; view.progressText = transferPhaseLabel(data);
    view.progressPhase = data.phase;
  };
  if (resumed) {
    renderMembers(plan); paintProgress(resumed); renderSelection();
    refreshEnvironment();
    if (resumed.error) {view.errorText = resumed.error; view.errorVisible = true;}
  } else await refreshPlan();
  const pollProgress = async () => {
    if (!dialog.isConnected) {clearInterval(progressTimer); return;}
    if (!plan || progressLoading) return;
    const id = plan.operation_id;
    progressLoading = true;
    try {
      const data = await request(operationStarted && bridge.HUB_MODE ? 'api/session/transfer/progress' : 'api/session/clone/progress', {uid, operation_id:id, target_node:view.targetNode});
      if (operationStarted && dialog.isConnected && plan?.operation_id === id) {
        paintProgress(data);
        if (!busy && !aborting && uncertain && data.phase === 'aborted') {
          uncertain = false; operationStarted = false; plan = null;
          await refreshPlan();
        }
      }
    } catch { /* The execution response reports actionable errors. */ }
    finally {progressLoading = false;}
  };
  progressTimer = setInterval(pollProgress, 1000);
  view.actions.confirm = async () => {
    if (busy || planning || aborting || !plan || blockedReason()) return;
    busy = true; view.errorVisible = false; renderSelection();
    const prepared = uncertain || await refreshPlan(true);
    if (!prepared || !plan || !dialog.isConnected) {busy = false; if (dialog.isConnected) renderSelection(); return;}
    const execution = ++executionSequence;
    operationStarted = true; view.errorVisible = false; renderSelection();
    if (bridge.HUB_MODE) paintProgress({phase:'planned'});
    try {
      const result = await request(crossMachine() ? 'api/session/transfer/clone' : 'api/session/clone', {
        uid, operation_id:plan.operation_id, ...(crossMachine() ? {target_node:view.targetNode} : {}),
      });
      if (execution !== executionSequence) return;
      if (result.phase !== 'complete' || !result.target_uid) throw new Error('复制未完成，请重试检查结果');
      close(); await bridge.loadSessions(true); await bridge.openSession(result.target_uid);
      bridge.showSessionStopNotice(result.mode === 'move' ? '整组移动完成。' : '整组复制完成，原会话已保留。');
    } catch (failure) {
      if (execution !== executionSequence) return;
      uncertain = failure.code !== 'move_cancelled';
      if (!uncertain) {operationStarted = false; plan = null; busy = false; view.progressVisible = false; await refreshPlan();}
      if (dialog.isConnected) {view.errorText = failure.message; view.errorVisible = true;}
    } finally {if (execution === executionSequence) {busy = false; if (dialog.isConnected) renderSelection();} refreshTransferTasks();}
  };
}


function transferPhaseLabel(task) {
  const labels = {planned:'准备迁移', preparing:'整理会话文件', checking:'检查目标目录与会话依赖', transferring:'传输历史', publishing:'发布历史', verifying:'验证历史与关系',
    failed:'复制失败，可重试', rollback_required:'恢复待处理',
    switching:'交接执行归属', releasing:'确认完成', retiring:'清理源端',
    cleanup_pending:'源端清理待重试', aborting:'撤回待完成', aborted:'已撤回', complete:'已完成'};
  let label = labels[task.phase] || '等待继续';
  if (task.phase === 'transferring' && task.bytes_total > 0)
    label += ` · ${bridge.fmtSize(task.bytes_sent)} / ${bridge.fmtSize(task.bytes_total)}`;
  return label;
}
let transferTasksLoading = false, tasksSignature = '';
async function refreshTransferTasks() {
  if (!bridge.HUB_MODE || transferTasksLoading) return;
  transferTasksLoading = true;
  try {
    const response = await fetch(bridge.appUrl('api/session/transfers'));
    if (!response.ok) return;
    const {operations} = await response.json();
    bridge.SessionDockShell.setTransfers(operations.length);
    const panel = bridge.$('#transfer-tasks-dialog');
    if (!panel) return;
    const name=id=>[...bridge.Nodes.machines,...bridge.Nodes.list].find(n=>n.id===id)?.name||'离线机器';
    const signature=JSON.stringify(operations);
    if (tasksSignature===signature) return;
    tasksSignature=signature;
    ui.tasks.rows=operations.map(task=>({task:markRaw(task),id:task.request.operation_id,title:task.plan?.sessions?.find(m=>m.uid===task.request.uid)?.title||'会话组',nodes:`${name(bridge.nodeOf(task.request.uid))} → ${name(task.request.target_node)}`,phase:transferPhaseLabel(task)}));
  } catch (error) {console.warn('迁移任务读取失败', error);}
  finally {transferTasksLoading = false;}
}
async function continueTransferTask(task) {
 const presentation=ui.tasks;presentation.pending.push(task.request.operation_id);
 try {
  const response=await fetch(bridge.appUrl('api/session/transfer/progress'),{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(task.request)});
  const data=await response.json();
  if(!response.ok || !data.plan)throw new Error(data.error?.message || data.error || '清单暂不可用');
  closeTransferTasks();await cloneSessionGroup(task.request.uid,data);
 }catch(failure){presentation.errorText=failure.message;}
 finally{presentation.pending=presentation.pending.filter(id=>id!==task.request.operation_id);}
}
function closeTransferTasks(){const panel=bridge.$('#transfer-tasks-dialog');panel?.close();ui.tasks=null;bridge.unmountTasks();}
async function openTransferTasks(){closeTransferTasks();tasksSignature='';ui.tasks=reactive({rows:[],pending:[],errorText:''});const panel=await bridge.mountTasks();panel.showModal();refreshTransferTasks();}
function initialize(){if(bridge.HUB_MODE){refreshTransferTasks();setInterval(()=>{if(!document.hidden)refreshTransferTasks();},5000);}}

return {initialize,transferUnavailableReason,paintTransferAvailability,cloneSessionGroup,transferPhaseLabel,refreshTransferTasks,openTransferTasks,closeTransferTasks,continueTransferTask};
}
