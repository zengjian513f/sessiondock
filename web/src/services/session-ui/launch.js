import { markRaw, nextTick } from 'vue'
import { createModelService } from './models.js'
/** @param {import('../../domain/session-ui/types').SessionUiPresentation} ui */
export function createLaunchController(bridge, ui) {
const {createModelPicker,modelCatalogs}=createModelService(bridge,ui);
const newNodeId = () => bridge.HUB_MODE ? ui.newNode : '';
const newNodeCapabilities = () => bridge.HUB_MODE ? bridge.Nodes.capabilities[newNodeId()] || {} : bridge.T;
const newDirsKey = () => bridge.HUB_MODE ? 'newDirs.' + newNodeId() : 'newDirs';
let bugReportToastTimer = 0;

const BUG_REPORT_SOURCES = { claude: 'Claude', codex: 'Codex', grok: 'Grok', opencode: 'OpenCode' };

function bugReportSource() {
  return ui.bugSource || 'codex';
}

// 与新建会话一样，四种 AI CLI 都可以做处理会话；记住上次的选择，处理机器上
// 缺少的命令置灰（中央站下按所选机器的能力表，单机按本机）。
function syncBugReportSources() {
  const remembered = bridge.store.get('bugReportSource', 'codex');
  const sources = bridge.HUB_MODE ? bridge.Nodes.capabilities[bugReportNode()]?.sources : bridge.T.sources;
  const known = sources && Object.keys(sources).length;
  ui.bugSources = Object.keys(BUG_REPORT_SOURCES).map(value => {
    const missing = known && !sources[value];
    return {value, disabled:!!missing, title:missing ? `${bugReportNodeName() || '本机'}找不到 ${value} 命令` : ''};
  });
  const fallback = ui.bugSources.find(input => input.value === remembered && !input.disabled)
    || ui.bugSources.find(input => !input.disabled);
  if (fallback) ui.bugSource = fallback.value;
}

// 处理会话的机器下拉与新建会话一样列出全部机器。报告只有一份：还有没发出
// 的草稿时回到草稿所在的机器；否则默认选问题所在的机器（当前会话的机器），
// 其次上次的选择，再次唯一筛选中的机器。
async function prepareBugReportNode() {
  if (!bridge.HUB_MODE) return;
  const origin = bugReportOriginNode();
  const selected = bridge.selectedNodeIds();
  const drafted = bridge.store.get('bugReportDraftNode', '');
  const preferred = [drafted, origin, bridge.store.get('bugReportNode', ''),
    selected.length === 1 ? selected[0] : ''].filter(Boolean);
  ui.bugNodes = bridge.Nodes.list.map(n=>({id:n.id,label:n.name + (bridge.Nodes.capabilities[n.id]?.enabled ? '' : '（离线）'),disabled:!bridge.Nodes.capabilities[n.id]?.enabled}));
  await nextTick();
  const usable = id => ui.bugNodes.some(o => o.id === id && !o.disabled);
  ui.bugNode = preferred.find(usable) || ui.bugNodes.find(o => !o.disabled)?.id || '';
  ui.bugNodeVisible = true;
  if (drafted && ui.bugNode !== drafted && bridge.Nodes.list.some(n => n.id === drafted)) {
    ui.bugError =
      `${bugReportNodeName(drafted)} 离线，上面没发出的报告草稿要等它恢复后才能打开`;
  }
}

// 记下正写着未发送内容的那份报告草稿在哪台机器上；清空或发出后忘掉。
function noteBugReportDraftNode() {
  const node = bridge.nodeOf(BUG_REPORT_DRAFT_UID), draft = bridge.composerDrafts.get(BUG_REPORT_DRAFT_UID);
  if (!node || !draft || draft.loading) return;
  if (draft.text.trim() || draft.quotes.length || draft.attachments.length) {
    bridge.store.set('bugReportDraftNode', node);
  } else if (bridge.store.get('bugReportDraftNode', '') === node) {
    bridge.store.set('bugReportDraftNode', '');
  }
}

function bugReportNodeName(id = bugReportNode()) {
  return bridge.HUB_MODE ? bridge.Nodes.list.find(n => n.id === id)?.name || '' : '';
}

function showBugReportToast(report, worker) {
  clearTimeout(bugReportToastTimer);
  const label = BUG_REPORT_SOURCES[worker?.source] || '处理';
  const where = worker?.node_name ? `到 ${worker.node_name}` : '';
  ui.bugToast = {report, worker:worker && markRaw(worker), text:`${report} 已保存${where}，${label} 处理会话正在启动`};
  bugReportToastTimer = setTimeout(dismissBugToast,20000);
}
function dismissBugToast() {clearTimeout(bugReportToastTimer);ui.bugToast=null;}
async function openBugToast() {const worker=ui.bugToast.worker;dismissBugToast();await bridge.loadTermList();const pending=(bridge.T.pending || []).find(item=>item.name===worker.name)||worker;await bridge.openPendingSession(pending);}

// 报告框的附件复用对话输入框那一套：同样的选择菜单、粘贴/拖放、[附件N]
// 引用，以及同一个上传接口。处理会话的 cwd 固定为仓库根目录，因此上传
// 先流式暂存到服务端私有目录，点击发送后才发布到仓库 sessiondock_attachments/。
// newComposerDraft 定义在下方的对话输入框段落，只能在运行时按需创建。
let BUG_REPORT_DRAFT_UID = '';
function bindBugReportDraft() {
  const node=bugReportNode();
  const key='reportDraftId.' + node;
  let id=bridge.store.get(key,'');
  if (!id) {id=crypto.randomUUID(); bridge.store.set(key,id);}
  BUG_REPORT_DRAFT_UID='report:' + (bridge.HUB_MODE ? node + '~' : '') + id;
}
let bugReportSending = false;
const bugReportDraftObject = () => bridge.composerDraft(BUG_REPORT_DRAFT_UID);

/** 下拉选的是跑处理会话的机器，不是另一份报告：正在写的描述、引用和附件
 *  跟着这次选择走，切换机器不清空输入框。服务端存储仍按机器分（附件的字节
 *  暂存在处理机器上），所以这里把内容搬到新机器的草稿上，再把原机器那份清
 *  空；目标机器上已有的服务端草稿由 hydrate 的早期编辑合并规则接上，两边
 *  都不丢。本页仍握着 File 的附件在新机器上重新暂存，原机器的暂存字节随
 *  清空后的草稿释放；重开页面后只剩服务端引用的卡片没有字节可重传，留在
 *  原机器的草稿里并在对话框上说明。返回要显示的提示文案。 */
function carryBugReportDraft(fromUid, toUid) {
  if (!fromUid || !toUid || fromUid === toUid || bugReportSending) return '';
  const from = bridge.composerDrafts.get(bridge.composerDraftOwner(fromUid));
  if (!from || from.handedOffSession) return '';
  const moving = from.attachments.filter(item => item.file instanceof Blob);
  const stranded = from.attachments.length - moving.length;
  if (!from.text && !from.quotes.length && !moving.length) return '';
  const to = bridge.composerDraft(toUid);
  if (!to) return '';
  const node = bugReportNode();
  to.text = to.text ? to.text + '\n' + from.text : from.text;
  to.quotes = [...to.quotes, ...from.quotes];
  to.attachments = [...to.attachments, ...moving];
  to.nextAttachmentNumber = Math.max(to.nextAttachmentNumber || 1, from.nextAttachmentNumber || 1);
  bridge.ensureComposerAttachmentNumbers(to);
  for (const field of ['requestId','requestText','report_prompt','report_text']) {
    delete to[field]; delete from[field];
  }
  from.attachments = from.attachments.filter(item => !moving.includes(item));
  from.text = ''; from.quotes = [];
  if (!from.attachments.length) from.nextAttachmentNumber = 1;
  const saved = bridge.persistComposerDraft(fromUid);
  for (const item of moving) {
    // 原机器上的暂存字节随清空后的草稿释放；新机器要的是一份新的上传。
    const previous = item.uploaded;
    if (item.cancelUpload) item.cancelUpload();
    item.uploaded = null; item.status = ''; item.error = '';
    bridge.discardStagedAttachment({uploaded: previous}, saved);
    const restage = () => {
      if (!to.attachments.includes(item)) return;
      // 被取消前已经落地的上传仍会写回 uploaded，再清一次才会重新暂存。
      if (item.uploaded) bridge.discardStagedAttachment(item, saved);
      item.uploaded = null; item.status = ''; item.error = '';
      bridge.stageComposerAttachment(item, toUid, {node, render: renderBugReportItems});
    };
    (item.staging || Promise.resolve()).then(restage, restage);
  }
  bridge.persistComposerDraft(toUid);
  if (!stranded) return '';
  const where = bugReportNodeName(bridge.nodeOf(fromUid)) || '原机器';
  return `${stranded} 个附件的文件只暂存在${where}，已留在那台机器的草稿里；`
    + '要随这份报告一起提交，请重新选择文件。';
}

function renderBugReportItems() {
  const draft=bugReportDraftObject();
  ui.bugText = draft.text;
  for (const attachment of draft.attachments) if (attachment.kind==='image') bridge.loadStagedComposerPreview(attachment,BUG_REPORT_DRAFT_UID,renderBugReportItems);
  ui.bugAttachments = draft.attachments.map(attachment=>markRaw({...attachment, raw:markRaw(attachment)}));
  ui.bugSending = bugReportSending;
  ui.bugStorageError = draft.storageError || '';
  ui.bugInputDisabled = bugReportSending || !!bugReportDraftObject().loading;
  ui.bugAddDisabled = bugReportSending || !!bugReportDraftObject().loading;
  // 发送中换机器会把正在提交的内容搬走；锁住下拉直到这一次提交结束。
  ui.bugNodeDisabled = bugReportSending;
  nextTick(()=>bridge.autoGrow(bridge.$('#bug-report-description')));
}

function addBugReportFiles(files) {
  const draft = bugReportDraftObject();
  const before = new Set(draft.attachments);
  bridge.addDraftFiles(draft, files);
  bridge.persistComposerDraft(BUG_REPORT_DRAFT_UID);
  for (const attachment of draft.attachments) {
    if (!before.has(attachment)) {
      bridge.stageComposerAttachment(attachment, BUG_REPORT_DRAFT_UID, {node: bugReportNode(), render: renderBugReportItems});
    }
  }
  renderBugReportItems();
}

function clearBugReportDraft() {
  const draft=bugReportDraftObject(), dropped=draft.attachments;
  for (const attachment of dropped) {
    if (attachment.preview) URL.revokeObjectURL(attachment.preview);
    if (attachment.cancelUpload) attachment.cancelUpload();
  }
  draft.text='';draft.attachments=[];draft.quotes=[];draft.nextAttachmentNumber=1;
  delete draft.requestId;delete draft.requestText;
  const saved=bridge.persistComposerDraft(BUG_REPORT_DRAFT_UID);
  for (const attachment of dropped) bridge.discardStagedAttachment(attachment, saved);
  renderBugReportItems();
}

// 处理会话（以及报告、附件）落在下拉里选中的机器上。
function bugReportNode() {
  return bridge.HUB_MODE ? ui.bugNode || '' : '';
}

// 问题所在的机器：当前会话的机器；没有会话时取唯一筛选中的机器；筛选着
// 多台机器又没选会话时说不清是哪台，按处理机器本身算。
function bugReportOriginNode() {
  if (!bridge.HUB_MODE) return '';
  const fromSession = bridge.nodeOf(bridge.S.sel);
  if (fromSession) return fromSession;
  const candidates = bridge.selectedNodeIds();
  return candidates.length === 1 ? candidates[0] : '';
}

// 报告里注明的问题机器；单机模式由服务端填主机名。
function bugReportOrigin(workerNode) {
  const uid = bridge.S.sel || '';
  if (!bridge.HUB_MODE) return {uid};
  const nodeId = bugReportOriginNode() || workerNode;
  return {node_id: nodeId, node_name: bugReportNodeName(nodeId), uid};
}

function bugReportNodeError(node) {
  if (!bridge.HUB_MODE) return '';
  if (!node) return '没有可用的机器：请先在顶部选择一台在线机器或打开一个会话';
  const info = bridge.Nodes.list.find(n => n.id === node);
  if (info?.online === false) {
    return `${info.name || '所选机器'} 离线，无法在该机器上保存报告；请换一台在线机器`;
  }
  if (!bridge.Nodes.capabilities[node]?.enabled) {
    return `${info?.name || '所选机器'} 未启用终端，无法启动处理会话；请换一台机器`;
  }
  return '';
}

// 处理机器不是问题机器时，先向问题机器要一份服务端上下文（会话行、发送账本、
// 终端画面、审计窗口），随报告交给处理机器；问题机器离线或抓取失败时不阻断
// 报告，只把原因写进诊断包。
async function captureBugReportContext(originNode, uid, terminalName) {
  try {
    const d = await bridge.post('api/bug-report/capture', {
      _node: originNode, uid, terminal_name: terminalName, page_id: bridge.TERM_PAGE_ID,
    });
    if (d.error) return {error: d.error};
    return d;
  } catch (failure) {
    return {error: `抓取失败：${failure.message || failure}`};
  }
}

function closeBugReportAttachMenu() {
  ui.bugAttachOpen = false;
  ui.bugAttachPadding = '';
}

async function openBugReportDialog() {
  const dialog = bridge.$('#bug-report-dialog');
  ui.bugError = '';
  ui.bugSubmitDisabled = typeof bridge.staleBuildShown !== 'undefined' && bridge.staleBuildShown;
  ui.bugSubmitLabel = '发送';
  setSendButtonBusy(bridge.$('#bug-report-go'), '');
  await prepareBugReportNode();
  bindBugReportDraft();
  renderBugReportItems();
  ui.bugText = bugReportDraftObject().text;
  bridge.hydrateComposerDraft(BUG_REPORT_DRAFT_UID).then(async () => {
    const handedOff=bugReportDraftObject().handedOffSession;
    if (handedOff) {
      showBugReportToast(handedOff.report_id,handedOff);
      bridge.store.set('reportDraftId.'+bugReportNode(),crypto.randomUUID());bindBugReportDraft();
      await bridge.hydrateComposerDraft(BUG_REPORT_DRAFT_UID);
    }
    if (!bugReportSending) {
      ui.bugText = bugReportDraftObject().text;
      renderBugReportItems();
      await nextTick();
      bridge.autoGrow(bridge.$('#bug-report-description'));
    }
  });
  syncBugReportSources();
  modelCatalogs.clear(); // 每次打开都读一遍：CLI 升级或换配置后列表会变
  BugReportModels.refresh();
  await nextTick();
  dialog.showModal();
  closeBugReportAttachMenu();
  await nextTick();
  bridge.autoGrow(bridge.$('#bug-report-description'));
  setTimeout(() => bridge.$('#bug-report-description').focus(), 0);
}

function bugNodeChanged(event) {
  if(event)ui.bugNode=event.target.value;
  const previous = BUG_REPORT_DRAFT_UID;
  bridge.store.set('bugReportNode', bugReportNode());
  bindBugReportDraft();
  const notice = carryBugReportDraft(previous, BUG_REPORT_DRAFT_UID);
  ui.bugText=bugReportDraftObject().text;
  renderBugReportItems();
  bridge.hydrateComposerDraft(BUG_REPORT_DRAFT_UID);
  syncBugReportSources();
  BugReportModels.refresh();
  ui.bugError = notice;
  nextTick(()=>bridge.autoGrow(bridge.$('#bug-report-description')));
}
async function bugAttachToggle(event) {
  event.stopPropagation();
  ui.bugAttachOpen = !ui.bugAttachOpen;
  if (!ui.bugAttachOpen) ui.bugAttachPadding = '';
  else {
    await nextTick();
    ui.bugAttachPadding = `${bridge.$('#bug-report-attach-menu').offsetHeight + 7}px`;
    await nextTick();
    bridge.$('#bug-report-add').scrollIntoView({block: 'nearest'});
  }
}

let bugReportBackdropPressed = false;
function bugReportBackdropHit(event) {
  const dialog = bridge.$('#bug-report-dialog');
  const rect = dialog.getBoundingClientRect();
  return event.target === dialog && (event.clientX < rect.left || event.clientX >= rect.right
    || event.clientY < rect.top || event.clientY >= rect.bottom);
}

function bugSourceChanged(event) {ui.bugSource=event.target.value;bridge.store.set('bugReportSource',bugReportSource());BugReportModels.refresh();}
function bugInput(event) {ui.bugText=event.target.value;bugReportDraftObject().text=ui.bugText;bridge.persistComposerDraft(BUG_REPORT_DRAFT_UID);bridge.autoGrow(event.target);}
function bugKeydown(e) {if(e.key==='Enter'&&!e.shiftKey&&!e.isComposing&&!bridge.MOBILE.matches){e.preventDefault();bridge.$('#bug-report-form').requestSubmit();}}
function bugPointerdown(event) {bugReportBackdropPressed=event.button===0&&bugReportBackdropHit(event);NewModels.outside(event);BugReportModels.outside(event);}
function bugPointercancel() {bugReportBackdropPressed=false;}
function bugClosed() {bugPointercancel();BugReportModels.close();}
function bugClick(event) {const dismiss=bugReportBackdropPressed&&bugReportBackdropHit(event);bugReportBackdropPressed=false;if(dismiss)bridge.$('#bug-report-dialog').close();if(!event.target.closest('#bug-report-dialog .attach-picker'))closeBugReportAttachMenu();}
function bugChooseFile(type) {closeBugReportAttachMenu();bridge.chooseAttachmentFiles(type,bridge.$('#bug-report-file'),addBugReportFiles);}
function bugFileChanged(event) {addBugReportFiles([...event.target.files]);event.target.value='';}
function bugPaste(event) {bridge.pasteAttachmentFiles(event,addBugReportFiles);}
function bugInsert(number) {bridge.insertComposerReference(number,bridge.$('#bug-report-description'));ui.bugText=bridge.$('#bug-report-description').value;}
function bugRetry(attachment) {bridge.stageComposerAttachment(attachment,BUG_REPORT_DRAFT_UID,{node:bugReportNode(),render:renderBugReportItems});}
function bugRemove(attachment) {if(attachment.cancelUpload){attachment.cancelUpload();return;}if(bugReportSending)return;const removed=bridge.removeDraftAttachment(bugReportDraftObject(),attachment.id);bridge.discardStagedAttachment(removed,bridge.persistComposerDraft(BUG_REPORT_DRAFT_UID));renderBugReportItems();}
function setSendButtonBusy(button, label) {
  if (!button) return;
  if(button.id==='bug-report-go'){ui.bugBusyLabel = label;return;}
  bridge.setComposerSendBusy(label);
}

function completeBugReportSubmission(data,node) {
  const oldUid=BUG_REPORT_DRAFT_UID,draft=bridge.composerDrafts.get(oldUid);
  if (draft) {
    for (const item of draft.attachments) if (item.preview) URL.revokeObjectURL(item.preview);
    bridge.composerSaveQueues.delete(draft);bridge.composerPendingSaves.delete(draft);bridge.composerSaving.delete(draft);
  }
  bridge.composerDrafts.delete(oldUid);bridge.composerHydrations.delete(oldUid);
  if (bridge.store.get('bugReportDraftNode', '') === node) bridge.store.set('bugReportDraftNode', '');
  bridge.store.set('reportDraftId.'+node,crypto.randomUUID());bindBugReportDraft();
  ui.bugText='';
  showBugReportToast(data.report_id,data.worker);bridge.$('#bug-report-dialog').close();
  void bridge.loadTermList();
}

async function submitBugReport(event) {
  event.preventDefault();
  if (bugReportSending) return;
  const reportText = ui.bugText;
  const description = reportText.trim();
  if (!description) {
    ui.bugError = '请先描述遇到的问题';
    bridge.$('#bug-report-description').focus();
    return;
  }
  const button = bridge.$('#bug-report-go');
  const attachments = [...bugReportDraftObject().attachments];
  const node = bugReportNode();
  const nodeError = bugReportNodeError(node);
  if (nodeError) {
    ui.bugError = nodeError;
    return;
  }
  const origin = bugReportOrigin(node);
  const remote = bridge.HUB_MODE && !!origin.node_id && origin.node_id !== node;
  if (typeof bridge.staleBuildShown !== 'undefined' && bridge.staleBuildShown) {
    ui.bugError = '页面已更新，请重新加载后再提交';
    return;
  }
  bugReportSending = true;
  ui.bugSubmitDisabled = true;
  setSendButtonBusy(button, '发送中');
  ui.bugAddDisabled = true;
  ui.bugError = '';
  renderBugReportItems();
  const snapshot = bridge.browserStateSnapshot('bug-report');
  bridge.browserAuditEvent('bug_report.requested', {
    ...snapshot.data, attachments: attachments.length,
    worker_node: node, origin_node: origin.node_id || '', remote,
  }, snapshot.content);
  try {
    bugReportDraftObject().text = reportText;
    await bridge.hydrateComposerDraft(BUG_REPORT_DRAFT_UID);
    const pending=bugReportDraftObject();
    const priorPayload=JSON.stringify({description,source:bugReportSource(),...BugReportModels.choice(),
      attachments:attachments.map(a=>({upload_id:a.uploaded?.upload_id,number:a.number})),origin});
    if (pending.requestId && pending.requestText===priorPayload) {
      const previous=await bridge.priorComposerSubmission(BUG_REPORT_DRAFT_UID,pending.requestId,true);
      if (previous?.worker) {completeBugReportSubmission(previous,node);return;}
    }
    if (!await bridge.persistComposerDraft(BUG_REPORT_DRAFT_UID)) throw new Error(bugReportDraftObject().storageError || '报告草稿保存失败');
    // 与对话发送一致：同一批附件共用一个编号目录，失败的附件保留在卡片上重试。
    const uploaded = [];
    let attachmentId = null;
    for (let i = 0; i < attachments.length; i++) {
      setSendButtonBusy(button, `上传 ${i + 1}/${attachments.length}`);
      if (attachments[i].staging) await attachments[i].staging;
      const result = await bridge.uploadComposerAttachment(
        attachments[i], BUG_REPORT_DRAFT_UID, attachmentId, { node, render: renderBugReportItems });
      attachmentId ||= result.attachment_id;
      uploaded.push({upload_id:result.upload_id,number:attachments[i].number});
    }
    const terminalName = bridge.takenOver(bridge.S.sel) || (bridge.T.uid === bridge.S.sel ? bridge.T.name : '') || '';
    let captured = null;
    if (remote) {
      setSendButtonBusy(button, '抓取中');
      captured = await captureBugReportContext(origin.node_id, bridge.S.sel || '', terminalName);
    }
    setSendButtonBusy(button, '提交中');
    const source = bugReportSource();
    bridge.store.set('bugReportSource', source);
    const reportDraft=bugReportDraftObject();
    const choice = BugReportModels.choice();
    const requestText=JSON.stringify({description,source,...choice,attachments:uploaded,origin});
    if (reportDraft.requestText!==requestText || !reportDraft.requestId) {
      reportDraft.requestText=requestText;reportDraft.requestId=crypto.randomUUID();
    }
    if (!await bridge.persistComposerDraft(BUG_REPORT_DRAFT_UID)) throw new Error(reportDraft.storageError || '报告草稿保存失败');
    // 远端抓取时会话与终端引用不再随请求下发：中央站要求 uid 与 _node 指向
    // 同一台机器，问题会话的引用改由 origin.uid 与 captured 携带。
    const d = await bridge.post('api/bug-report', {
      ...(bridge.HUB_MODE ? {_node: node} : {}),
      draft_uid:BUG_REPORT_DRAFT_UID,draft_revision:reportDraft.revision,request_id:reportDraft.requestId,
      description, uid: remote ? '' : (bridge.S.sel || ''), page_id: bridge.TERM_PAGE_ID, source, ...choice,
      terminal_name: remote ? '' : terminalName, snapshot, attachments: uploaded,
      origin, ...(captured ? {captured} : {}),
      cols: Math.max(80, bridge.T.term?.cols || 120), rows: Math.max(24, bridge.T.term?.rows || 36),
    });
    if (d.error) {
      ui.bugError = d.error;
      return;
    }
    completeBugReportSubmission(d,node);
  } catch (failure) {
    ui.bugError = `提交失败：${failure.message || failure}`;
  } finally {
    bugReportSending = false;
    ui.bugSubmitDisabled = false;
    setSendButtonBusy(button, '');
    ui.bugAddDisabled = false;
    renderBugReportItems();
  }
}
async function prepareNewNode() {
  if (!bridge.HUB_MODE) return;
  const previous = ui.newNode;
  const selected = bridge.selectedNodeIds();
  const preferred = selected.length === 1 ? selected[0]
    : bridge.nodeOf(bridge.S.sel) || previous || bridge.store.get('newNode', '');
  ui.newNodes = bridge.Nodes.list.map(n=>({id:n.id,label:n.name+(bridge.Nodes.capabilities[n.id]?.enabled?'':'（离线）'),disabled:!bridge.Nodes.capabilities[n.id]?.enabled}));
  await nextTick();
  if (ui.newNodes.some(o => o.id === preferred && !o.disabled)) ui.newNode = preferred;
  else ui.newNode = ui.newNodes.find(o => !o.disabled)?.id || '';
  ui.newNodeVisible = true;
}

// 切换机器时，输入框里的目录若在新机器上也存在就原样保留；只有不存在
// （或无法确认）时才换成新机器的默认目录。用户在检查期间改了输入则不动。
let newCwdCheck = 0;
async function newNodeHasDir(path) {
  const value = path.length > 1 ? path.replace(/\/+$/, '') : path;
  if (value === '/' || value === '~') return true;
  if (!canCompleteCwd(value)) return false;
  try {
    const params = new URLSearchParams({ path: value, limit: '50' });
    const response = await fetch(bridge.appUrl(`api/term/complete-dir?${params}`), { cache: 'no-store' });
    const data = await response.json();
    return response.ok && Array.isArray(data.directories) && data.directories.includes(value + '/');
  } catch { return false; }
}

function refreshNewNodeFields(keepCwd = '') {
  closeCwdPicker();
  const cap = newNodeCapabilities();
  cwdCompletion.common = commonSessionDirs();
  ui.newSources = ['claude','codex','grok','opencode','shell'].map(value => {
    const disabled = !cap.sources?.[value];
    const name = bridge.SOURCES[value]?.name || (value === 'shell' ? 'SSH' : value);
    return {value, disabled, title:disabled ? `${name}：此机器未安装或未配置该客户端` : name};
  });
  const checked = ui.newSources.find(input => input.value === ui.newSource);
  if (!checked || checked.disabled) {
    const first = ui.newSources.find(input => !input.disabled);
    if (first) {ui.newSource = first.value; NewModels.refresh();}
  }
  const selected = bridge.S.sessions.find(s => s.uid === bridge.S.sel && (!bridge.HUB_MODE || s.node_id === newNodeId()));
  const fallback = selected?.cwd || bridge.store.get(newDirsKey(), [])[0]
    || cwdCompletion.common[0]?.cwd || cap.home || '';
  const check = ++newCwdCheck;
  if (keepCwd && keepCwd !== fallback && cap.enabled) {
    ui.newCwd = keepCwd;
    const node = newNodeId();
    newNodeHasDir(keepCwd).then(exists => {
      if (exists || check !== newCwdCheck || node !== newNodeId() || ui.newCwd.trim() !== keepCwd) return;
      ui.newCwd = fallback;
    });
  } else ui.newCwd = fallback;
  ui.newError = '';
  ui.newSubmitDisabled = !cap.enabled;
  renderCommonCwdOptions();
}


function newNodeChanged(event){if(event)ui.newNode=event.target.value;bridge.store.set('newNode',newNodeId());refreshNewNodeFields(ui.newCwd.trim());NewModels.refresh();}
function suggestedSessionDir(cwd) {
  const path = String(cwd || '').replace(/\/+$/, '') || '/';
  // CLI/SDK 经常在这些易失根目录里生成一次性测试会话。它们仍属于会话
  // 历史，但不该因一次自动任务污染“最近使用”的新建目录建议。
  return path.startsWith('/') && !['/tmp', '/var/tmp', '/dev/shm'].some(
    root => path === root || path.startsWith(root + '/'));
}

function commonSessionDirs() {
  const dirs = new Map();
  for (const s of bridge.S.sessions) {
    if (bridge.HUB_MODE && s.node_id !== newNodeId()) continue;
    const cwd = String(s.cwd || '');
    if (!suggestedSessionDir(cwd)) continue;
    const row = dirs.get(cwd) || { cwd, count: 0, updated: '' };
    row.count++;
    if ((s.updated || '') > row.updated) row.updated = s.updated || '';
    dirs.set(cwd, row);
  }
  for (const [i, cwd] of bridge.store.get(newDirsKey(), []).entries()) {
    if (!cwd?.startsWith('/')) continue;
    const row = dirs.get(cwd) || { cwd, count: 0, updated: '' };
    row.recent = 20 - i;
    dirs.set(cwd, row);
  }
  const home = newNodeCapabilities().home;
  if (home && !dirs.has(home)) dirs.set(home, { cwd: home, count: 0, updated: '' });
  return [...dirs.values()].sort((a, b) =>
    (b.recent || 0) - (a.recent || 0) || b.count - a.count
    || b.updated.localeCompare(a.updated) || a.cwd.localeCompare(b.cwd));
}

const CWD_COMPLETION_DELAY = 120;
const cwdCompletion = {
  timer: null, abort: null, sequence: 0,
  rows: [], completions: [], forValue: '', active: -1, mode: 'common', common: [],
};

function canCompleteCwd(value) {
  const path = String(value || '').trim();
  return path.startsWith('/') || path === '~' || path.startsWith('~/');
}

function cancelCwdCompletionRequest() {
  if (cwdCompletion.timer) clearTimeout(cwdCompletion.timer);
  cwdCompletion.timer = null;
  cwdCompletion.abort?.abort();
  cwdCompletion.abort = null;
  cwdCompletion.sequence++;
}

function closeCwdPicker() {
  cancelCwdCompletionRequest();
  cwdCompletion.rows = [];
  cwdCompletion.completions = [];
  cwdCompletion.forValue = '';
  cwdCompletion.active = -1;
  ui.cwdRows = [];
  ui.cwdVisible = false;
  ui.cwdActive = -1;
}

function cwdOption(path, meta = '', kind = 'recent') {
  return { path: String(path || ''), meta: String(meta || ''), kind };
}

function cwdPathKey(path) {
  const value = String(path || '');
  return value === '/' ? value : value.replace(/\/+$/, '');
}

function matchingRecentCwdOptions(value = '') {
  const query = String(value || '').trim().toLocaleLowerCase();
  return cwdCompletion.common
    .filter(row => !query || String(row.cwd || '').toLocaleLowerCase().includes(query))
    .map(row => cwdOption(row.cwd, row.count ? `${row.count} 个会话` : '', 'recent'));
}

function renderCwdOptions(value, recentRows, completionRows = [], completionNote = '') {
  const rawRecent = recentRows.map(row => typeof row === 'string'
    ? cwdOption(row, '', 'recent') : row);
  const rawCompletions = completionRows.map(row => typeof row === 'string'
    ? cwdOption(row, '', 'completion') : row);
  const completionFirst = String(value || '').startsWith('/');
  const seen = new Set();
  const unique = rows => rows.filter(row => {
    const key = cwdPathKey(row.path);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
  const completions = completionFirst ? unique(rawCompletions) : [];
  const recent = unique(rawRecent);
  if (!completionFirst) completions.push(...unique(rawCompletions));
  const options = completionFirst
    ? [...completions, ...recent] : [...recent, ...completions];
  cwdCompletion.mode = value ? 'matching' : 'common';
  cwdCompletion.rows = options.map(row => row.path);
  // recent 与建议中的同一路径只画一次，但它仍是文件系统补全候选；
  // Tab 计算公共前缀时不能因为视觉去重而把它漏掉。
  cwdCompletion.completions = rawCompletions.map(row => row.path);
  cwdCompletion.forValue = value;
  cwdCompletion.active = -1;
  const rows = [];
  ui.cwdTitle = value ? '匹配目录' : '最近使用';
  ui.cwdVisible = true;
  const section = label => rows.push({section:label});
  const note = message => rows.push({note:message});
  const addOption = (row, index) => rows.push({...row,index});
  if (!value) {
    recent.forEach(addOption);
    if (!recent.length) note('还没有使用过的目录');
  } else {
    let offset = 0;
    const addGroup = (label, rows, empty = '') => {
      if (!rows.length && !empty) return;
      section(label);
      rows.forEach((row, index) => addOption(row, offset + index));
      offset += rows.length;
      if (!rows.length && empty) note(empty);
    };
    if (completionFirst) {
      addGroup('补全建议', completions, completionNote);
      addGroup('最近匹配', recent);
    } else {
      addGroup('最近匹配', recent);
      addGroup('补全建议', completions, completionNote);
    }
    if (!recent.length && !completions.length && !completionNote) note('没有匹配的目录');
  }
  ui.cwdRows = rows;
  ui.cwdActive = -1;
  ui.cwdStatus = options.length
    ? (value ? `${recent.length} 个最近匹配，${completions.length} 个补全建议`
             : `${recent.length} 个最近目录`)
    : (completionNote || '没有匹配的目录');
}

function renderCommonCwdOptions() {
  renderCwdOptions('', matchingRecentCwdOptions());
}

function setCwdCompletionActive(step) {
  const rows = cwdCompletion.rows;
  if (!rows.length) return;
  const old = cwdCompletion.active;
  const next = old < 0
    ? (step > 0 ? 0 : rows.length - 1)
    : (old + step + rows.length) % rows.length;
  cwdCompletion.active = next;
  const options = [...bridge.$('#new-cwd-options').querySelectorAll('[data-cwd-option]')];
  ui.cwdActive = next;
  const option = options[next];
  option.scrollIntoView({ block: 'nearest' });
}

function setCwdValue(value, refresh = true) {
  const input = bridge.$('#new-cwd');
  ui.newCwd = value;
  ui.newError = '';
  input.focus();
  nextTick(() => input.setSelectionRange(value.length, value.length));
  if (refresh) scheduleCwdCompletions();
}

function longestCommonPrefix(values) {
  if (!values.length) return '';
  let prefix = values[0];
  for (const value of values.slice(1)) {
    let i = 0;
    while (i < prefix.length && i < value.length && prefix[i] === value[i]) i++;
    prefix = prefix.slice(0, i);
    if (!prefix) break;
  }
  return prefix;
}

function applyCwdTabCompletion() {
  const input = bridge.$('#new-cwd');
  if (!cwdCompletion.rows.length) return;
  if (cwdCompletion.active >= 0) {
    setCwdValue(cwdCompletion.rows[cwdCompletion.active]);
    return;
  }
  const rows = cwdCompletion.completions;
  if (!rows.length) return;
  if (rows.length === 1) {
    setCwdValue(rows[0]);
    return;
  }
  const value = ui.newCwd.trim();
  const prefix = longestCommonPrefix(rows);
  if (prefix.length > value.length) {
    setCwdValue(prefix, false);
    cwdCompletion.forValue = prefix;
    ui.cwdStatus =
      `已补全公共前缀，仍有 ${rows.length} 个补全建议`;
  }
}

async function loadCwdCompletions(complete = false) {
  cancelCwdCompletionRequest();
  const input = bridge.$('#new-cwd');
  const value = ui.newCwd.trim();
  const recent = matchingRecentCwdOptions(value);
  if (bridge.SessionDockCapabilities.config.backend === 'rust' && !bridge.SessionDockCapabilities.allows('terminal_complete_dir')) {
    renderCwdOptions(value, recent, [], '请填写已配置白名单中的现有工作目录；不会自动创建目录。');
    return;
  }
  if (!canCompleteCwd(value)) {
    renderCwdOptions(value, recent);
    return;
  }
  const controller = new AbortController();
  const sequence = ++cwdCompletion.sequence;
  cwdCompletion.abort = controller;
  try {
    const params = new URLSearchParams({ path: value });
    const response = await fetch(bridge.appUrl(`api/term/complete-dir?${params}`),
      { signal: controller.signal, cache: 'no-store' });
    const data = await response.json();
    if (sequence !== cwdCompletion.sequence || ui.newCwd.trim() !== value) return;
    const rows = response.ok && Array.isArray(data.directories)
      ? data.directories.filter(path => typeof path === 'string' && canCompleteCwd(path)).slice(0, 24)
      : [];
    renderCwdOptions(value, recent, rows, response.ok
      ? (rows.length ? '' : '没有补全建议')
      : (data.error || '目录补全暂不可用'));
    if (complete) applyCwdTabCompletion();
  } catch (error) {
    if (error.name !== 'AbortError' && sequence === cwdCompletion.sequence) {
      renderCwdOptions(value, recent, [], '目录补全暂不可用');
    }
  } finally {
    if (cwdCompletion.abort === controller) cwdCompletion.abort = null;
  }
}

function scheduleCwdCompletions() {
  cancelCwdCompletionRequest();
  const value = ui.newCwd.trim();
  if (!value) {
    renderCommonCwdOptions();
    return;
  }
  const recent = matchingRecentCwdOptions(value);
  if (!canCompleteCwd(value)) {
    renderCwdOptions(value, recent);
    return;
  }
  renderCwdOptions(value, recent, [], '正在查找目录…');
  cwdCompletion.timer = setTimeout(() => loadCwdCompletions(false), CWD_COMPLETION_DELAY);
}


const NewModels = createModelPicker('new', {
  source: () => ui.newSource || '',
  node: () => newNodeId(), storeKey: 'newModel'});
const BugReportModels = createModelPicker('bug-report', {
  source: () => bugReportSource(), node: () => bugReportNode(), storeKey: 'bugReportModel'});

async function openNewSessionDialog() {
  const dialog = bridge.$('#new-session-dialog');
  closeCwdPicker();
  newCreateAttempt = null;
  modelCatalogs.clear(); // 每次打开都读一遍：CLI 升级或换配置后列表会变
  await prepareNewNode();
  refreshNewNodeFields();
  NewModels.refresh();
  await nextTick();
  dialog.showModal();
  renderCommonCwdOptions();
  setTimeout(() => { bridge.$('#new-cwd').focus(); bridge.$('#new-cwd').select(); }, 0);
}


let newCreateAttempt = null;
function newSessionRequestId(source, cwd) {
  const key = JSON.stringify([newNodeId(), source, cwd, NewModels.model, NewModels.effort]);
  if (newCreateAttempt?.key !== key) newCreateAttempt = {key,
    rows: bridge.termRows(),
    id: globalThis.crypto?.randomUUID?.() || `${Date.now()}-${Math.random().toString(36).slice(2)}`};
  return newCreateAttempt.id;
}

async function createNewSession(e) {
  e.preventDefault();
  const source = ui.newSource;
  const cwd = ui.newCwd.trim();
  ui.newError = '';
  if (!source) { ui.newError = '没有可用的会话类型'; return; }
  if (!cwd) { ui.newError = '请选择启动目录'; return; }
  ui.newSubmitDisabled = true;
  ui.newSubmitLabel = '创建中…';
  const dirsKey = newDirsKey();
  try {
    const requestId = newSessionRequestId(source, cwd);
    const request = { source, cwd, cols: 120, rows: newCreateAttempt.rows,
      request_id: requestId, ...(bridge.HUB_MODE ? {_node: newNodeId()} : {}),
      ...NewModels.choice() };
    let d = await bridge.post('api/term/create', request);
    if (d.needs_create) {
      const target = String(d.cwd || cwd);
      if (!await bridge.appConfirm(`启动目录不存在：\n${target}\n\n是否创建该目录并继续？`)) {
        bridge.$('#new-cwd').focus();
        return;
      }
      ui.newSubmitLabel = '创建目录中…';
      d = await bridge.post('api/term/create', { ...request, cwd: target, create_cwd: true });
    }
    if (d.error) { ui.newError = d.error; return; }
    const recent = [d.cwd, ...bridge.store.get(dirsKey, []).filter(x => x !== d.cwd)].slice(0, 8);
    bridge.store.set(dirsKey, recent);
    bridge.$('#new-session-dialog').close();
    await bridge.openPendingSession(d);
    // The backend instance is ready; list refresh must not block the conversation.
    void bridge.loadTermList();
  } catch (err) {
    ui.newError = err.message || '创建失败';
  } finally {
    ui.newSubmitDisabled = false;
    ui.newSubmitLabel = '创建';
  }
}


function cwdKeydown(e) {
  if (e.isComposing) return;
  if (e.key === 'Tab' && !e.shiftKey && canCompleteCwd(e.currentTarget.value)) {
    e.preventDefault();
    if (cwdCompletion.mode === 'matching' && cwdCompletion.completions.length
        && cwdCompletion.forValue === e.currentTarget.value.trim()) {
      applyCwdTabCompletion();
    } else {
      loadCwdCompletions(true);
    }
  } else if (e.key === 'ArrowDown' && cwdCompletion.rows.length) {
    e.preventDefault();
    setCwdCompletionActive(1);
  } else if (e.key === 'ArrowUp' && cwdCompletion.rows.length) {
    e.preventDefault();
    setCwdCompletionActive(-1);
  } else if (e.key === 'Enter' && cwdCompletion.active >= 0) {
    e.preventDefault();
    setCwdValue(cwdCompletion.rows[cwdCompletion.active]);
  }
}
function cwdChoose(e) {
  const option = e.target.closest('[data-cwd-option]');
  if (!option) return;
  e.preventDefault();
  const path = cwdCompletion.rows[Number(option.dataset.cwdOption)];
  if (path) setCwdValue(path);
}
let newSessionBackdropPressed = false;
function newSessionBackdropHit(event) {
  const dialog = bridge.$('#new-session-dialog');
  const rect = dialog.getBoundingClientRect();
  return event.target === dialog && (event.clientX < rect.left || event.clientX >= rect.right
    || event.clientY < rect.top || event.clientY >= rect.bottom);
}

function newPointerdown(event) {newSessionBackdropPressed = event.button === 0 && newSessionBackdropHit(event);}
function newPointercancel() {newSessionBackdropPressed = false;}
function newBackdropClick(event) {const dismiss = newSessionBackdropPressed && newSessionBackdropHit(event); newSessionBackdropPressed = false; if (dismiss) bridge.$('#new-session-dialog').close();}
function newClosed() {newPointercancel();closeCwdPicker();NewModels.close();}
function cwdInput(event) {ui.newCwd=event.target.value;ui.newError='';scheduleCwdCompletions();}
function newSourceChanged(event) {ui.newSource=event.target.value;NewModels.refresh();}

function initialize() {
 document.addEventListener('DOMContentLoaded',()=>{bridge.$('#new-node').onchange=null;});
 bridge.bindFileDrop(bridge.$('#bug-report-form'), addBugReportFiles);
 window.addEventListener('resize',()=>{if(bridge.$('#bug-report-dialog').open)bridge.autoGrow(bridge.$('#bug-report-description'));});
}
return {prepareNewNode,refreshNewNodeFields,newSourceChanged,newNodeId,newNodeCapabilities,newDirsKey,canCompleteCwd,get cwdCompletion(){return cwdCompletion;},newNodeChanged,formatSize:bridge.fmtSize,kindIcon:bridge.composerKindIcon,initialize,NewModels,BugReportModels,openNewSessionDialog,createNewSession,cwdKeydown,cwdChoose,cwdInput,newPointerdown,newPointercancel,newBackdropClick,newClosed,commonSessionDirs,renderCommonCwdOptions,closeCwdPicker,showBugReportToast,dismissBugToast,openBugToast,openBugReportDialog,bugNodeChanged,bugSourceChanged,bugInput,bugKeydown,bugPointerdown,bugPointercancel,bugClosed,bugClick,bugAttachToggle,bugChooseFile,bugFileChanged,bugPaste,bugInsert,bugRetry,bugRemove,submitBugReport,renderBugReportItems,addBugReportFiles,clearBugReportDraft,carryBugReportDraft,noteBugReportDraftNode,bindBugReportDraft,bugReportDraftObject,bugReportNode,bugReportSource,setSendButtonBusy,get BUG_REPORT_DRAFT_UID(){return BUG_REPORT_DRAFT_UID;},get bugReportSending(){return bugReportSending;}};
}
