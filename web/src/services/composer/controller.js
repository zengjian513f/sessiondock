
import * as SessionUi from '../../migration/session-ui'

import {sessiondockCli} from '../../domain/runtime/cli.js'
import {viewKey} from '../../domain/runtime/messages.js'
import {pendingUid} from '../../domain/runtime/pending'
import {fmtSize,fmtTime} from '../../domain/runtime/format.js'
import {composerHistoryStamp,nativeComposerHistory,ensureComposerAttachmentNumbers,composerFileKind,composerKindIcon,composerInputStatus,composerInputAllowsSend,screenMenuRevision,clipboardAttachmentFiles,clipboardDirectoryNames,clipboardCsvFile} from '../../domain/composer/input.js'

import {nextTick} from 'vue'
// Scoped composer service. Explicit dependencies keep the existing native
// controllers, identities and request contracts; Vue owns the composer projection.
export function createComposerController(bridge, ui) {
const COMPOSER_MAX_FILES = 12;

const COMPOSER_MAX_FILE_BYTES = 512 * 1024 * 1024;

const ATTACH_ACCEPT = { image: 'image/*', video: 'video/*', audio: 'audio/*', file: '' };

async function chooseAttachmentFiles(type, input, addFiles) {
  input.accept = ATTACH_ACCEPT[type] ?? '';
  input.dataset.kind = type;
  input.removeAttribute('capture');
  if (type === 'file' && /Android/i.test(navigator.userAgent)) {
    // Chromium's Android chooser treats octet-stream as all openable files,
    // without the camera/recorder intents added for an empty accept or */*.
    input.accept = 'application/octet-stream';
    if (typeof window.showOpenFilePicker === 'function') {
      let handles;
      try {
        handles = await window.showOpenFilePicker({multiple: true, excludeAcceptAllOption: false});
      } catch (error) {
        if (error.name === 'AbortError') return;
        if (error.name !== 'NotSupportedError' && error.name !== 'SecurityError') {
          await SessionUi.appAlert('选择文件失败：' + (error.message || String(error)));
          return;
        }
      }
      if (handles) {
        try {
          const files = await Promise.all(handles.map(handle => handle.getFile()));
          if (files.length) addFiles(files);
        } catch (error) {
          await SessionUi.appAlert('读取所选文件失败：' + (error.message || String(error)));
        }
        return;
      }
    }
  }
  input.click();
}

const composerDrafts = new Map();

const composerDraftAliases = new Map();

const composerInputHistoryCache = new Map();

const composerHistoryPicker = {
  open: false, uid: null, items: [], index: -1, seq: 0,
};

let composerUid = null;

let composerDraftSeq = 0;

let lastMessageSelection = '';

let lastMessageSelectionUid = null;

const conversationSendEnabled = () => bridge.capabilities.config.conversation_send === true;

const newComposerDraft = () => ({text:'', attachments:[], quotes:[], nextAttachmentNumber:1,
  revision:0, editVersion:0, savedVersion:0});

const composerHydrations = new Map();

let composerDraftWrites = Promise.resolve();

const composerSaveQueues = new Map();

function composerDraftOwner(uid) {
  const seen = new Set();
  while (composerDraftAliases.has(uid) && !seen.has(uid)) {
    seen.add(uid); uid = composerDraftAliases.get(uid);
  }
  return uid;
}

function restoreComposerDraftRecord(record, uid) {
  const draft = Object.assign(newComposerDraft(), record || {});
  delete draft.saved;
  // Optional/nullable fields in old server records must retain editor defaults.
  draft.text ??= '';
  draft.attachments ??= [];
  draft.quotes ??= [];
  for (const attachment of draft.attachments) {
    attachment.status = ''; attachment.preview = '';
    // The server resolved this draft through the requested session identity.
    // Report handoff/restart can leave an older UID in its stored metadata.
    if (uid && attachment.uploaded?.upload_id) {
      attachment.uploaded = {...attachment.uploaded, uid, node:bridge.runtime().nodes.nodeOf(uid) || ''};
    }
  }
  return ensureComposerAttachmentNumbers(draft);
}

function composerDraft(uid = composerUid, create = true) {
  uid = composerDraftOwner(uid);
  if (!uid) return null;
  if (!composerDrafts.has(uid) && create) composerDrafts.set(uid, newComposerDraft());
  if (create) hydrateComposerDraft(uid);
  return composerDrafts.get(uid) || null;
}

function composerDraftRecord(draft, uid) {
  return {text:draft.text, quotes:draft.quotes.map(item => ({...item})),
    attachments:draft.attachments.map(item => ({id:item.id, number:item.number, kind:item.kind,
      uploaded:item.uploaded, file:{name:item.file.name, size:item.file.size,
        type:item.file.type, lastModified:item.file.lastModified}})),
    nextAttachmentNumber:draft.nextAttachmentNumber, requestId:draft.requestId,
    requestText:draft.requestText, session:{...draft.session, uid},
    ...(draft.report_prompt ? {report_prompt:draft.report_prompt,report_text:draft.report_text} : {})};
}

async function priorComposerSubmission(uid,id,report=false) {
  const query=new URLSearchParams({uid,[report?'report_request_id':'request_id']:id});
  const response=await bridge.runtime().network.fetch(bridge.environment.appUrl('api/session/conversation?'+query),{cache:'no-store'});
  const data=await response.json();
  if (response.status===404) return null;
  if (!response.ok || data.error) throw new Error(data.error || `HTTP ${response.status}`);
  return data;
}

function acceptComposerServerRevision(draft,row) {
  if (!row || row.revision<=draft.revision) return;
  const value=row.value;
  const empty=!value?.text && !value?.attachments?.length && !value?.quotes?.length;
  const same=value?.text===draft.text
    && JSON.stringify((value?.attachments || []).map(a=>a.id))===JSON.stringify(draft.attachments.map(a=>a.id))
    && JSON.stringify(value?.quotes || [])===JSON.stringify(draft.quotes);
  if (empty || same) draft.revision=row.revision;
}

function applyCliState(uid, cli, {status = true} = {}) {
  const draft = composerDrafts.get(composerDraftOwner(uid));
  if (!draft || !cli || typeof cli !== 'object') return;
  if (Number.isFinite(cli.observed_at) && Number.isFinite(draft.cli?.observed_at)
      && cli.observed_at < draft.cli.observed_at) return;
  const priorBusy = draft.cli?.instance?.busy;
  draft.cli = cli;
  if (status && cli.input && bridge.terminal().takenOver(uid)) {
    updateComposerInputStatus(uid, {ok: cli.input.state === 'ready', input: cli.input});
  }
  if (composerDraftOwner(composerUid) === composerDraftOwner(uid)) bridge.pendingStage().renderQueuedSends(composerUid);
  if (priorBusy !== cli.instance?.busy && composerDraftOwner(composerUid) === composerDraftOwner(uid))
    renderComposerInputStatus();
  bridge.status().paintTurn(uid);
}

async function dismissQueuedSend(uid, requestId) {
  try {
    await bridge.post().post('api/session/conversation/queued/dismiss', {uid, request_id: requestId});
  } catch { /* The next CLI-state packet shows whether it is still queued. */ }
  const draft = composerDrafts.get(composerDraftOwner(uid));
  if (Array.isArray(draft?.cli?.queued)) {
    draft.cli.queued = draft.cli.queued.filter(item => item.request_id !== requestId);
  }
  bridge.pendingStage().renderQueuedSends(uid);
}

async function consumeComposerSubmission(uid,text,attachments,quotes) {
  const owner=composerDraftOwner(uid),draft=composerDrafts.get(owner);
  if (!draft) return;
  delete draft.requestId;delete draft.requestText;delete draft.report_prompt;delete draft.report_text;
  if (draft.text===text) draft.text='';
  const files=new Set(attachments.map(a=>a.id)),quoted=new Map(quotes.map(q=>[q.id,q.text]));
  for (const item of draft.attachments.filter(a=>files.has(a.id))) {
    if (item.preview) URL.revokeObjectURL(item.preview);
  }
  draft.attachments=draft.attachments.filter(a=>!files.has(a.id));
  draft.quotes=draft.quotes.filter(q=>quoted.get(q.id)!==q.text);
  if (!draft.text && !draft.attachments.length && !draft.quotes.length) draft.nextAttachmentNumber=1;
  // SEND already durably consumed this revision. Save any remaining/new edits
  // in the background; network latency here must not delay the successful UI.
  persistComposerDraft(owner);refreshComposerDraft(owner);
}

async function readServerComposerDraft(uid) {
  const url = 'api/session/conversation?' + new URLSearchParams({uid});
  const traceId = crypto.randomUUID();
  const fields = {uid, traceId};
  const started = performance.now(), timeoutMs = 12000;
  let phase = 'headers', status = null, headersMs = null;
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  bridge.runtime().audit.browserAuditEvent?.('http.request.started', {url, method:'GET'}, null, fields);
  try {
    const response = await bridge.runtime().network.fetch(bridge.environment.appUrl(url), {cache:'no-store', signal:controller.signal,
      headers:{'X-SessionDock-Trace':traceId, 'X-SessionDock-Page':bridge.environment.AUDIT_PAGE_ID,
        'X-SessionDock-Build':bridge.environment.BUILD_ID}});
    status = response.status;
    headersMs = Math.round(performance.now() - started);
    phase = 'body';
    bridge.runtime().audit.browserAuditEvent?.('http.response.headers', {url, status, headers_ms:headersMs}, null, fields);
    const data = await response.json();
    phase = 'response';
    if (!response.ok || data.error) throw new Error(data.error || `HTTP ${response.status}`);
    // Record transport metadata only: draft text and attachments stay private.
    bridge.runtime().audit.browserAuditEvent?.('http.response.received', {url, status, ok:true,
      headers_ms:headersMs, duration_ms:Math.round(performance.now() - started)}, null, fields);
    return data.draft;
  } catch (error) {
    if (controller.signal.aborted) {
      error = new Error('草稿读取超时，连接恢复后会自动重试');
      error.name = 'TimeoutError';
    }
    bridge.runtime().audit.browserAuditEvent?.('http.request.failed', {url, error:String(error), phase, status,
      headers_ms:headersMs, timeout_ms:timeoutMs, online:navigator.onLine,
      visibility:document.visibilityState, duration_ms:Math.round(performance.now() - started)},
    null, {...fields, severity:'warning'});
    throw error;
  } finally { clearTimeout(timer); }
}

let legacyComposerDatabase;

async function oldComposerDatabase() {
  if (legacyComposerDatabase !== undefined) return legacyComposerDatabase;
  legacyComposerDatabase = null;
  if (!globalThis.indexedDB?.databases) return null;
  const databases = await indexedDB.databases();
  if (!databases.some(db => db.name === bridge.environment.STORAGE_PREFIX + 'composer-drafts')) return null;
  legacyComposerDatabase = await new Promise((resolve, reject) => {
    const request = indexedDB.open(bridge.environment.STORAGE_PREFIX + 'composer-drafts');
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
  return legacyComposerDatabase;
}

async function importLegacyComposer(uid) {
  let record = bridge.preferences.get('composerDraft.' + uid, null);
  const db = await oldComposerDatabase();
  if (db) {
    const indexed = await new Promise((resolve, reject) => {
      const request = db.transaction('drafts').objectStore('drafts').get(uid);
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    if (indexed && (!record || (+indexed.revision || 0) > (+record.revision || 0))) record = indexed;
  }
  const oldQueue=bridge.preferences.get('queuedMessages',[]).find(([key])=>key===uid)?.[1];
  if (oldQueue?.length) {
    const imported=await bridge.post().post('api/session/conversation/import',{uid,value:{legacy_queue:oldQueue}});
    if (imported.error) throw new Error(imported.error);
    const remaining=bridge.preferences.get('queuedMessages',[]).filter(([key])=>key!==uid);
    if (remaining.length) bridge.preferences.set('queuedMessages',remaining);
    else localStorage.removeItem(bridge.environment.STORAGE_PREFIX+'queuedMessages');
  }
  if (!record || record.removed) return null;
  const converted=structuredClone(record);
  const attachments=[...(converted.attachments || []),
    ...(converted.saved || []).flatMap(item=>item.attachments || [])];
  if (db) for (const item of attachments) {
    if (item.uploaded?.upload_id) continue;
    const file=await new Promise((resolve,reject) => {
      const request=db.transaction('files').objectStore('files').get(uid+'\0'+item.id);
      request.onsuccess=()=>resolve(request.result);request.onerror=()=>reject(request.error);
    });
    if (!(file instanceof Blob)) continue;
    const url=new URL(bridge.environment.appUrl('api/session/conversation/attachment'));
    url.searchParams.set('uid',uid);url.searchParams.set('id','legacy-'+item.id);
    url.searchParams.set('name',item.file.name);
    const response=await bridge.runtime().network.fetch(url,{method:'POST',headers:{'Content-Type':item.file.type || 'application/octet-stream'},body:file});
    const uploaded=await response.json();
    if (!response.ok || uploaded.error) throw new Error(uploaded.error || `HTTP ${response.status}`);
    item.uploaded={...uploaded,uid};
  }
  const result=await bridge.post().post('api/session/conversation/import',{uid,value:converted});
  if (result.error) throw new Error(result.error);
  // Delete browser originals only after every existing File has a durable
  // server reference. Unknown/missing bytes keep their original recovery store.
  if (attachments.every(item=>item.uploaded?.upload_id)) {
    if (db) await new Promise((resolve,reject) => {
      const transaction=db.transaction(['drafts','files'],'readwrite');
      transaction.objectStore('drafts').delete(uid);
      const prefix=uid+'\0';
      const request=transaction.objectStore('files').openKeyCursor(IDBKeyRange.bound(prefix,prefix+'\uffff'));
      request.onsuccess=()=>{const cursor=request.result;if (cursor){transaction.objectStore('files').delete(cursor.key);cursor.continue();}};
      transaction.oncomplete=resolve;transaction.onabort=transaction.onerror=()=>reject(transaction.error);
    });
    localStorage.removeItem(bridge.environment.STORAGE_PREFIX+'composerDraft.'+uid);
    const remaining=bridge.preferences.get('composerDraftUids',[]).filter(key=>key!==uid);
    if (remaining.length) bridge.preferences.set('composerDraftUids',remaining);
    else localStorage.removeItem(bridge.environment.STORAGE_PREFIX+'composerDraftUids');
  }
  return {record:converted,db};
}

function hydrateComposerDraft(uid, retry = false) {
  uid = composerDraftOwner(uid);
  if (composerHydrations.has(uid)) {
    if (!retry || !composerDrafts.get(uid)?.loadFailed || composerDrafts.get(uid)?.loading)
      return composerHydrations.get(uid);
    composerHydrations.delete(uid);
  }
  if (!conversationSendEnabled()) return Promise.resolve();
  const task = (async () => {
    const draft = composerDrafts.get(uid);
    if (!draft) return;
    draft.loading = true;
    try {
      const legacy = await importLegacyComposer(uid);
      const row = await readServerComposerDraft(uid);
      // A page that has never saved adopts the server revision even when
      // typing began before this read returned; keeping revision 0 would get
      // every later save refused. A page that has saved keeps its CAS
      // baseline, and the save path rebases on a conflict.
      if (!draft.savedVersion) draft.revision = row.revision;
      if (uid.startsWith('report:') && row.value?.session?.kind==='bug-report'
          && row.value.session.uid && !row.value.session.uid.startsWith('report:')) {
        draft.handedOffSession=row.value.session;return;
      }
      if (!draft.editVersion && row.value && !row.value.removed) {
        for (const field of ['requestId','requestText','report_prompt','report_text']) delete draft[field];
        Object.assign(draft, restoreComposerDraftRecord(row.value, uid), {revision:row.revision});
        if (legacy?.db) await Promise.all(draft.attachments.map(async item => {
          if (item.uploaded?.upload_id) return;
          const file = await new Promise((resolve, reject) => {
            const request = legacy.db.transaction('files').objectStore('files').get(uid + '\0' + item.id);
            request.onsuccess = () => resolve(request.result);
            request.onerror = () => reject(request.error);
          });
          if (file) {
            item.file = new File([file], item.file.name, {type:item.file.type,
              lastModified:item.file.lastModified});
            if (item.kind === 'image') item.preview = URL.createObjectURL(item.file);
          }
        }));
      } else if (draft.editVersion && !draft.savedVersion && row.value && !row.value.removed) {
        mergeEarlyComposerEdit(draft, restoreComposerDraftRecord(row.value, uid), uid);
      }
      draft.storageError = '';
      draft.loadFailed = false;
    } catch (error) {
      draft.storageError = '服务端草稿读取失败，当前输入保留：' + (error.message || error);
      draft.loadFailed = true;
    } finally {
      draft.loading = false;
      refreshComposerDraft(composerDraftOwner(uid)); syncComposerUnloadProtection();
    }
  })();
  composerHydrations.set(uid, task);
  return task;
}

function mergeEarlyComposerEdit(draft, server, uid) {
  if (server.text && !draft.text.startsWith(server.text)) {
    draft.text = server.text + (draft.text ? '\n' + draft.text : '');
  }
  draft.attachments = [...server.attachments,
    ...draft.attachments.filter(a => !server.attachments.some(b => b.id === a.id))];
  draft.quotes = [...server.quotes, ...draft.quotes.filter(q => !server.quotes.some(s => s.id === q.id))];
  draft.nextAttachmentNumber = Math.max(draft.nextAttachmentNumber || 1, server.nextAttachmentNumber || 1);
  if (!draft.session && server.session) draft.session = server.session;
  ensureComposerAttachmentNumbers(draft);
  queueComposerSave(draft, uid);
}

function adoptServerDraft(draft, row, uid) {
  const next = restoreComposerDraftRecord(row.value, uid);
  next.attachments = next.attachments.map(a => {
    const local = draft.attachments.find(b => a.id === b.id);
    if (!local) return a;
    local.number = a.number;
    if (a.kind) local.kind = a.kind;
    if (a.uploaded?.upload_id && (!local.uploaded?.upload_id
        || local.uploaded.upload_id === a.uploaded.upload_id)) local.uploaded = a.uploaded;
    return local;
  });
  for (const a of draft.attachments) if (a.preview && !next.attachments.some(b => a.id === b.id)) URL.revokeObjectURL(a.preview);
  for (const field of ['requestId','requestText','report_prompt','report_text']) delete draft[field];
  Object.assign(draft, next, {revision: row.revision, editVersion: draft.editVersion,
    savedVersion: draft.editVersion, storageError: ''});
  return draft;
}

let composerFollowBusy = false, composerFollowedAt = 0;

async function followServerDraft(uid, revision = null) {
  if (bridge.runtime().network.paused) return false;
  const owner = composerDraftOwner(uid), draft = composerDrafts.get(owner);
  if (!draft || !conversationSendEnabled() || draft.loading || draft.loadFailed || draft.handedOffSession
      || composerSending || composerFollowBusy || composerSaving.has(draft) || composerPendingSaves.has(draft)
      || draft.editVersion !== draft.savedVersion) return false;
  if (revision !== null && revision <= draft.revision) return false;
  if (revision === null && performance.now() - composerFollowedAt < 1000) return false;
  composerFollowBusy = true;
  const version = draft.editVersion;
  try {
    const row = await readServerComposerDraft(uid);
    composerFollowedAt = performance.now();
    if (!row || row.revision <= draft.revision || draft.editVersion !== version
        || composerSaving.has(draft) || composerSending) return false;
    adoptServerDraft(draft, row, uid);
    refreshComposerDraft(owner); syncComposerUnloadProtection();
    return true;
  } catch {
    return false; // The next poll, focus or switch reads again.
  } finally {
    composerFollowBusy = false;
  }
}

function refreshComposerDraft(uid) {
  if (composerUid === uid) {
    ui.text=composerDrafts.get(uid)?.text || '';
    renderComposerItems(); nextTick(()=>autoGrow($('#cinput')));
  }
  if (uid === bridge.launch().BUG_REPORT_DRAFT_UID) {
    bridge.launch().renderBugReportItems(); bridge.launch().noteBugReportDraftNode();
  }
}

const composerPendingSaves=new Map();

const composerSaving=new Set();

function queueComposerSave(draft, uid) {
  composerPendingSaves.set(draft,{uid,version:++draft.editVersion,value:composerDraftRecord(draft,uid)});
  syncComposerUnloadProtection();
}

function persistComposerDraft(uid = composerUid) {
  uid = composerDraftOwner(uid);
  const draft = composerDrafts.get(uid);
  if (!draft) return Promise.resolve(false);
  if (uid === bridge.launch().BUG_REPORT_DRAFT_UID) bridge.launch().noteBugReportDraftNode();
  // Draft storage remains available across deployments; only SEND is build-gated.
  queueComposerSave(draft, uid);
  if (composerSaving.has(draft)) return composerSaveQueues.get(draft);
  composerSaving.add(draft);
  const task=(async () => {
    await new Promise(resolve=>setTimeout(resolve,150));
    try {
      await hydrateComposerDraft(uid, true);
      if (draft.loadFailed) return false; // Keep the read error; never wrap it again.
      if (!conversationSendEnabled()) throw new Error('草稿保存未启用');
      while (composerPendingSaves.has(draft)) {
        const pending=composerPendingSaves.get(draft);composerPendingSaves.delete(draft);
        let data=await bridge.post().post('api/session/conversation',{uid:pending.uid,revision:draft.revision,value:pending.value}, {timeoutMs:12000});
        for (let attempt=0;data.code==='draft_revision' && attempt<2;attempt++) {
          // Another page saved first. This page is the one still editing, so
          // its input wins: rebase onto the server revision and save again.
          const row=await readServerComposerDraft(pending.uid);
          if (row.revision>draft.revision) draft.revision=row.revision;
          data=await bridge.post().post('api/session/conversation',{uid:pending.uid,revision:draft.revision,value:pending.value}, {timeoutMs:12000});
        }
        if (data.reload) throw new Error(data.error || '页面已更新，请重新加载后再提交');
        if (data.error) throw new Error(data.error);
        draft.revision=data.draft.revision;draft.savedVersion=pending.version;draft.storageError='';
      }
      return true;
    } catch (error) {
      draft.storageError='服务端草稿保存失败，当前输入保留：'+(error.message || error);
      return false;
    } finally {
      composerSaving.delete(draft);syncComposerUnloadProtection();
      const owner = composerDraftOwner(uid);
      if (owner === bridge.launch().BUG_REPORT_DRAFT_UID) {
        renderSavedComposerInputs($('#bug-report-items'), draft);
      } else if (composerDraftOwner(composerUid) === owner) {
        renderSavedComposerInputs($('#compose-items'), draft);
      }
    }
  })();
  composerSaveQueues.set(draft,task);
  composerDraftWrites=Promise.all([...composerSaveQueues.values()]);
  return task;
}

let composerRecoveryBusy = false;

async function recoverComposerDrafts() {
  if (bridge.sleep().sleeping || bridge.runtime().network.reason === 'login') return;
  if (composerRecoveryBusy || document.hidden || !navigator.onLine || composerSending) return;
  composerRecoveryBusy = true;
  try {
    for (const [uid, draft] of composerDrafts) {
      if (!draft.storageError || draft.loading || composerSaving.has(draft)
          || draft.handedOffSession || draft.requestId) continue;
      if (draft.loadFailed) await hydrateComposerDraft(uid, true);
      if (!draft.loadFailed && draft.editVersion > draft.savedVersion) await persistComposerDraft(uid);
    }
  } finally { composerRecoveryBusy = false; }
}

const composerUnloadWarning = event => {event.preventDefault(); event.returnValue = '';};

let composerUnloadProtected = false;

function syncComposerUnloadProtection() {
  const pending = [...composerDrafts.values()].some(draft => draft.storageError
    || draft.editVersion > draft.savedVersion
    || draft.attachments.some(item => !item.uploaded?.upload_id && item.file instanceof Blob));
  if (pending === composerUnloadProtected) return;
  composerUnloadProtected = pending;
  if (pending) window.addEventListener('beforeunload', composerUnloadWarning);
  else window.removeEventListener('beforeunload', composerUnloadWarning);
}

async function prepareComposerReload() {
  await Promise.all([...composerDrafts].map(([uid, draft]) =>
    composerSaving.has(draft) ? composerSaveQueues.get(draft)
      : draft.editVersion > draft.savedVersion ? persistComposerDraft(uid) : null));
  syncComposerUnloadProtection();
  return !composerUnloadProtected && !composerSending && !bridge.launch().bugReportSending;
}

function renderSavedComposerInputs(box, draft) {
  if(box?.id==='compose-items'){ui.storageError=draft.storageError || '';return;}
  const existing = box.querySelector(':scope > .draft-save-error[role="alert"]');
  if (!draft.storageError) { existing?.remove(); return; }
  if (existing) { existing.textContent = draft.storageError; return; }
  const error = bridge.dom().el('div', 'draft-save-error', draft.storageError);
  error.setAttribute('role','alert'); box.append(error);
}

const composerServerRecoveries = new Map();

function recoverServerComposerDrafts() {
  if (!conversationSendEnabled() || document.hidden) return Promise.resolve();
  const nodes = bridge.environment.HUB_MODE ? bridge.runtime().state.nodes.list : [{id: '', online: true}];
  const tasks = [];
  for (const node of nodes) {
    let state = composerServerRecoveries.get(node.id);
    if (!state) {
      state = {done: false, request: null, failures: 0, retryAt: 0, online: node.online};
      composerServerRecoveries.set(node.id, state);
    }
    if (state.online === false && node.online === true) state.retryAt = 0;
    state.online = node.online;
    if (state.request) { tasks.push(state.request); continue; }
    if (state.done || node.online === false || Date.now() < state.retryAt) continue;
    state.request = recoverServerComposerNode(node.id, state).finally(() => { state.request = null; });
    tasks.push(state.request);
  }
  return Promise.all(tasks);
}

async function recoverServerComposerNode(node, state) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 12000);
  try {
    const url = bridge.environment.appUrl('api/session/conversation/drafts' + (node ? '?node=' + node : ''));
    const response = await bridge.runtime().network.fetch(url, {cache: 'no-store', signal: controller.signal});
    const data = await response.json();
    if (!response.ok || data.error) throw new Error(data.error || `HTTP ${response.status}`);
    let changed = false;
    for (const row of data.drafts || []) {
      if (!row.uid || row.uid.startsWith('report:') || composerDrafts.has(row.uid)) continue;
      const draft = restoreComposerDraftRecord(row.draft.value, row.uid); draft.revision = row.draft.revision;
      composerDrafts.set(row.uid, draft);
      changed = true;
    }
    state.done = true;
    state.failures = 0;
    // Apply successful peers immediately, without waiting for an offline peer.
    if (changed && bridge.runtime().state.catalog.sig && typeof bridge.sidebarView().renderSide === 'function') bridge.sidebarView().renderSide();
  } catch {
    state.retryAt = Date.now() + Math.min(300000, 30000 * 2 ** Math.min(state.failures++, 4));
  } finally { clearTimeout(timer); }
}

function deleteComposerDraftStorage(uid) {
  localStorage.removeItem(bridge.environment.STORAGE_PREFIX + 'composerDraft.' + uid);
  const remaining = bridge.preferences.get('composerDraftUids', []).filter(key => key !== uid);
  if (remaining.length) bridge.preferences.set('composerDraftUids', remaining);
  else localStorage.removeItem(bridge.environment.STORAGE_PREFIX + 'composerDraftUids');
}

function pendingStartedAt(info, fallback = Date.now() / 1000) {
  const started = Number(info?.started);
  if (Number.isFinite(started) && started > 0) return started;
  const created = Date.parse(info?.created || '');
  return Number.isFinite(created) ? created / 1000 : fallback;
}

function rememberComposerSession(draft, info) {
  const previous = draft.session?.name === info.name ? Number(draft.session.started) : 0;
  draft.session = Object.fromEntries(['uid', 'name', 'source', 'cwd', 'node_id', 'node_name',
    'record_id', 'launch_id', 'instance_id', 'title', 'kind', 'report_id']
    .filter(key => info[key] != null).map(key => [key, info[key]]));
  // Once term/list drops the exited instance, the draft-retained sidebar row is
  // sorted by this stamp; keep the first one recorded for the same instance.
  const started = previous > 0 ? previous : pendingStartedAt(info, 0);
  if (started > 0) draft.session.started = started;
}

function syncComposerDraftBindings() {
  for (const [uid, draft] of [...composerDrafts]) {
    if (!uid.startsWith('tmux:') || !draft.session?.instance_id) continue;
    const row = (bridge.terminal().state.list || []).find(row => row.name === draft.session.name
      && row.instance_id === draft.session.instance_id && row.uid);
    if (row) migrateComposerDraft(uid, row.uid);
  }
}

async function composerHistoryItems(uid) {
  const entry = bridge.runtime().cache.cache.get(viewKey(uid));
  let items;
  if (entry && !entry.partial) {
    items = nativeComposerHistory(entry.msgs);
  } else {
    const stamp = composerHistoryStamp(entry);
    const cached = composerInputHistoryCache.get(uid);
    if (cached?.stamp === stamp) {
      items = cached.items.map(item => ({ ...item }));
    } else {
      const query = new URLSearchParams({uid});
      const response = await bridge.runtime().network.fetch(bridge.environment.appUrl(`api/session/input-history?${query}`));
      const data = await response.json();
      if (!response.ok || data.error) throw new Error(data.error || `HTTP ${response.status}`);
      items = (data.history || []).map((item, index) => ({
        id: `native-${index}`, text: String(item.text || ''), ts: item.ts || null,
      })).filter(item => item.text.trim());
      const resultStamp = `${data.end || 0}:${data.version?.head || ''}:${data.anchor || ''}`;
      composerInputHistoryCache.set(uid, {
        stamp: resultStamp, items: items.map(item => ({ ...item })),
      });
    }
  }
  return items.map(item => ({ ...item }));
}

function closeComposerHistory() {
  composerHistoryPicker.open=false; composerHistoryPicker.uid=null; composerHistoryPicker.seq++;
  ui.history={...composerHistoryPicker,state:'ready',error:''};
}

function setComposerHistoryIndex(index) {
  const picker=composerHistoryPicker;
  if (!picker.open || !picker.items.length) return;
  picker.index=Math.max(0,Math.min(index,picker.items.length-1));
  ui.history={...ui.history,...picker};
  requestAnimationFrame(()=>$('#input-history')?.querySelector(`[data-history-index="${picker.index}"]`)?.scrollIntoView({block:'nearest'}));
}

function renderComposerHistory(state='ready') {
  ui.history={...composerHistoryPicker,state,error:''};
  if(state!=='loading' && composerHistoryPicker.items.length) setComposerHistoryIndex(composerHistoryPicker.index);
}

async function openComposerHistory() {
  const ta = $('#cinput');
  const uid = composerUid;
  if (!uid || composerSending || ta.value !== '') return;
  closeAttachMenu();
  const seq = ++composerHistoryPicker.seq;
  Object.assign(composerHistoryPicker, {
    open: true, uid, items: [], index: -1,
  });
  const entry = bridge.runtime().cache.cache.get(viewKey(uid));
  const seed = nativeComposerHistory(entry?.msgs);
  if (seed.length) {
    composerHistoryPicker.items = seed;
    composerHistoryPicker.index = seed.length - 1;
    renderComposerHistory(entry?.partial ? 'refreshing' : 'ready');
  } else {
    renderComposerHistory('loading');
  }
  try {
    const items = await composerHistoryItems(uid);
    if (!composerHistoryPicker.open || composerHistoryPicker.uid !== uid
        || composerHistoryPicker.seq !== seq || composerUid !== uid || ta.value !== '') return;
    const selected = composerHistoryPicker.items[composerHistoryPicker.index];
    composerHistoryPicker.items = items;
    const preserved = selected ? items.findLastIndex(item =>
      item.text === selected.text && item.ts === selected.ts) : -1;
    composerHistoryPicker.index = preserved >= 0 ? preserved : items.length - 1;
    renderComposerHistory();
  } catch (error) {
    if (!composerHistoryPicker.open || composerHistoryPicker.seq !== seq) return;
    composerHistoryPicker.items = [];
    composerHistoryPicker.index = -1;
    renderComposerHistory();
    ui.history={...ui.history,error:`读取失败：${error.message || error}`};
  }
}

function acceptComposerHistory() {
  const picker = composerHistoryPicker;
  const item = picker.items[picker.index];
  if (!picker.open || !item) return false;
  const ta = $('#cinput');
  closeComposerHistory();
  ta.value = item.text;
  ui.text=item.text;
  ta.dispatchEvent(new Event('input', {bubbles: true}));
  ta.focus();
  ta.setSelectionRange(ta.value.length, ta.value.length);
  return true;
}

function migrateComposerDraft(fromUid, toUid) {
  if (!fromUid || !toUid || fromUid === toUid) return;
  const draft = composerDrafts.get(fromUid);
  if (!draft) return;
  // This is only called after the terminal instance is explicitly bound.
  // Never concatenate an already edited destination with another draft.
  const target = composerDrafts.get(toUid);
  if (target?.editVersion && (target.text || target.attachments.length || target.quotes.length)) return;
  composerDrafts.set(toUid, draft);
  for (const [key, value] of bridge.questionRuntime().screenMenuTextDrafts) {
    if (!key.startsWith(`${fromUid}\0`)) continue;
    bridge.questionRuntime().screenMenuTextDrafts.set(`${toUid}\0${key.slice(fromUid.length + 1)}`, value);
    bridge.questionRuntime().screenMenuTextDrafts.delete(key);
  }
  composerDraftAliases.set(fromUid, toUid);
  composerDrafts.delete(fromUid);
  if (composerHydrations.has(fromUid)) composerHydrations.set(toUid, composerHydrations.get(fromUid));
  for (const attachment of draft.attachments) {
    if (attachment.uploaded?.uid === fromUid) attachment.uploaded.uid = toUid;
  }
  if (composerUid === fromUid) composerUid = toUid;
}

function switchComposerDraft(uid) {
  const ta = $('#cinput');
  if (composerUid === uid) return;
  closeComposerHistory();
  composerUid = uid;
  const draft = composerDraft(uid, !!uid);
  if (draft) {draft.inputStatus = null; draft.inputPrompt = null; draft.inputAnswer = null;
    draft.inputProbe = (draft.inputProbe || 0) + 1;}
  ta.value = draft?.text || '';
  ui.text=ta.value;
  renderComposerItems();
  autoGrow(ta);
  if (uid) {
    followServerDraft(uid); // Already hydrated: pick up edits saved elsewhere.
    pollComposerInput();
  }
}

function sessionComposerEnded(uid = bridge.runtime().state.selection.sel) {
  if (!uid) return false;
  if (typeof bridge.terminal().state !== 'undefined' && bridge.terminal().state.ended?.has(uid)) return true;
  const name = String(uid).startsWith('tmux:')
    ? String(uid).slice(5)
    : (typeof bridge.terminal().takenOver === 'function' ? bridge.terminal().takenOver(uid) : null);
  const view = name && typeof bridge.terminal().state !== 'undefined' ? bridge.terminal().state.views?.get(name) : null;
  if (view?.ended || view?.retired) return true;
  const rows = typeof bridge.terminal().state === 'undefined' ? [] : [
    ...(typeof bridge.runtime().pending.pendingTmuxSessions === 'function' ? bridge.runtime().pending.pendingTmuxSessions() : []),
    ...(bridge.terminal().state.pending || []),
  ];
  const row = rows.find(item => item.uid === uid
    || (typeof pendingUid === 'function' && pendingUid(item.name) === uid));
  return !!row && ['exited', 'failed', 'stopping'].includes(bridge.terminal().pendingPhase(row));
}

function renderComposer() {
  const shell = typeof bridge.terminal().sessionIsPtyOnly === 'function' && bridge.terminal().sessionIsPtyOnly(bridge.runtime().state.selection.sel);
  const enabled=conversationSendEnabled() || shell;
  const name = enabled && bridge.runtime().nodes.sessionTerminalEnabled(bridge.runtime().state.selection.sel) ? bridge.terminal().takenOver(bridge.runtime().state.selection.sel) : null;
  const pending = enabled && String(bridge.runtime().state.selection.sel || '').startsWith('tmux:');
  const receipt = pending && (bridge.terminal().state.pending || []).find(row => pendingUid(row.name) === bridge.runtime().state.selection.sel);
  const restartable = conversationSendEnabled() && !shell
    && ['exited', 'failed'].includes(receipt?.state)
    && receipt?.binding?.state !== 'confirmed';
  // A subagent view shares S.sel with its parent but has no CLI of its own, so
  // neither the parent's editor nor its input notice belongs there.
  const show = !bridge.runtime().state.selection.agent
    && (restartable || (!sessionComposerEnded(bridge.runtime().state.selection.sel) && !!(name || pending)));
  ui.visible=show;
  const first = typeof bridge.terminal().sessionTerminalFirst === 'function' && bridge.terminal().sessionTerminalFirst(bridge.runtime().state.selection.sel);
  $('#right')?.classList.toggle('shell-session', !!shell);
  $('#right')?.classList.toggle('terminal-first', !!first);
  switchComposerDraft(show ? bridge.runtime().state.selection.sel : null);
  if (show) syncComposerMode();
  if (first && typeof bridge.terminal().layoutTermPane === 'function') bridge.terminal().layoutTermPane();
}

function autoGrow(ta) {
  ta.style.height = 'auto';
  const wanted = ta.scrollHeight;
  ta.style.height = Math.min(180, Math.max(36, wanted)) + 'px';
  ta.style.overflowY = wanted > 180 ? 'auto' : 'hidden';
}

function syncComposerMode() {
  ui.placeholder=bridge.environment.MOBILE.matches ? '输入内容' : '输入内容，Enter 发送，Shift+Enter 换行';
  $('#bug-report-description').placeholder=bridge.environment.MOBILE.matches ? '描述遇到的问题' : '描述遇到的问题，Enter 发送，Shift+Enter 换行';
  autoGrow($('#cinput'));
}

async function sendToSession(text, keys, uid = bridge.runtime().state.selection.sel, media = [], options = {}) {
  const name = bridge.terminal().takenOver(uid);
  if (!name) return false;
  if ((text || options.requestId) && !keys && conversationSendEnabled()) {
    try {
      const draft = composerDraft(uid);
      const data = await bridge.post().post('api/session/conversation/send', {
        uid, name, text, request_id:options.requestId || crypto.randomUUID(),
        draft_revision:options.draftRevision, attachments:options.attachments || [],
        quotes:options.quotes || [], lease:bridge.terminal().termSendLease(name).lease || null,
      });
      if (data.error) {updateComposerInputStatus(uid, data); throw new Error(data.error);}
      acceptComposerServerRevision(draft,data.draft);
      bridge.runtime().state.live.live.add(uid); bridge.runtime().state.live.liveTmux.add(uid); bridge.runtime().state.selection.lastSync = 0; bridge.runtime().state.selection.syncGap = 350;
      bridge.status().paintLive();
      return true;
    } catch (error) {
      await SessionUi.appAlert('发送失败，输入保留：' + (error.message || error));
      return false;
    }
  }
  // Without the conversation service (a node lacking its prerequisites, or
  // a PTY-only shell) text is a bracketed paste, then Enter once the CLI took it.
  let d;
  try {
    const rawText = !!text;
    const body = bridge.terminal().termInputBody(name, keys ? { name, keys, uid }
      : rawText ? { name, paste: text, uid } : { name, text });
    if (!body) {
      await SessionUi.appAlert('发送失败: 此后端未启用该会话的可靠发送；控制台键盘和快捷键仍可直接输入。');
      return false;
    }
    d = await bridge.post().post('api/term/send', body);
    if (rawText && !d.error) {
      // The CLI may briefly show a paste-burst marker. Sending Enter in
      // the same tick can be swallowed while that marker is active.
      await new Promise(resolve => setTimeout(resolve, 600));
      d = await bridge.post().post('api/term/send', bridge.terminal().termInputBody(name, { name, keys: ['Enter'], uid }));
    }
  } catch (e) {
    await SessionUi.appAlert('发送失败: ' + (e.message || e));
    return false;
  }
  if (d.error) {
    await SessionUi.appAlert('发送失败: ' + d.error);
    return false;
  }
  bridge.runtime().state.live.live.add(uid);            // 发完立刻按最快节奏拉新消息
  bridge.runtime().state.live.liveTmux.add(uid);
  bridge.status().paintLive();
  bridge.runtime().state.selection.syncGap = 350;
  bridge.runtime().state.selection.lastSync = 0;
  return true;
}

function closeAttachMenu() { ui.menu=false; }

function renderAttachmentCards(box, attachments,
  { onInsert, onRemove, onRetry = null, disabled = false, uid = null, render = () => {} }) {
  box.replaceChildren();
  for (const attachment of attachments) {
    const card = bridge.dom().el('div', `draft-card ${attachment.status || ''}`);
    card.dataset.draftId = attachment.id;
    card.title = `点击插入 [附件${attachment.number}]`;
    card.onclick = e => {
      if (!e.target.closest('.draft-remove')) onInsert(attachment.number);
    };
    const thumb = bridge.dom().el('span', 'draft-thumb');
    if (attachment.kind === 'image') loadStagedComposerPreview(attachment, uid, render);
    if (attachment.kind === 'image' && attachment.preview) {
      const image = document.createElement('img');
      image.src = attachment.preview;
      image.alt = '';
      thumb.appendChild(image);
    } else {
      thumb.textContent = composerKindIcon(attachment.kind);
    }
    const info = bridge.dom().el('span', 'draft-info');
    const name = document.createElement('b');
    name.textContent = attachment.uploaded?.name || attachment.file?.name || 'attachment';
    const meta = document.createElement('small');
    const ref = `[附件${attachment.number}]`;
    const kindName = attachment.kind === 'file' ? '文件'
      : ({ image: '图片', video: '视频', audio: '音频' }[attachment.kind]);
    const summary = `${ref} · ${kindName} · ${fmtSize(attachment.file?.size ?? attachment.uploaded?.size ?? 0)}`;
    meta.textContent = attachment.status === 'uploading' ? `${summary} · ${attachment.progress || 0}%`
      : attachment.status === 'queued' ? `${summary} · 等待上传`
        : attachment.status === 'failed' ? `${summary} · ${attachment.error || '上传失败'}`
          : summary;
    info.append(name, meta);
    if (attachment.status === 'failed' && onRetry && attachment.file instanceof Blob) {
      const retry = bridge.dom().el('button', 'draft-retry', '重试');
      retry.type = 'button'; retry.title = '重新上传';
      retry.disabled = disabled;
      retry.onclick = e => { e.stopPropagation(); onRetry(attachment); };
      info.appendChild(retry);
    }
    const remove = bridge.dom().el('button', 'draft-remove', '×');
    remove.type = 'button';
    remove.title = remove.ariaLabel = attachment.cancelUpload ? '取消上传' : '移除附件';
    remove.disabled = disabled && !attachment.cancelUpload;
    remove.onclick = e => {
      e.stopPropagation();
      if (attachment.cancelUpload) attachment.cancelUpload();
      else onRemove(attachment.id);
    };
    card.append(thumb, info, remove);
    box.appendChild(card);
  }
}

function composerUsesInputStatus(uid = composerUid) {
  return conversationSendEnabled()
    && !(typeof bridge.terminal().sessionIsPtyOnly === 'function' && bridge.terminal().sessionIsPtyOnly(uid));
}

function updateComposerInputStatus(uid, data) {
  const owner = composerDraftOwner(uid), draft = composerDrafts.get(owner);
  if (!draft) return;
  draft.inputProbe = (draft.inputProbe || 0) + 1;
  const status = composerInputStatus(data);
  // Watch packets carry only cli.input, whereas CHECK explicitly carries
  // prompt (including null). A partial status must not erase a live card or
  // cancel its pending answer before the clicked control's own fresh CHECK.
  const prompt = status.code === 'cli_question'
    ? Object.hasOwn(data || {}, 'prompt')
      ? ['folder_trust','screen_menu'].includes(data?.prompt?.kind) ? data.prompt : null
      : draft.inputPrompt || null
    : null;
  const changed = JSON.stringify(draft.inputStatus) !== JSON.stringify(status)
    || JSON.stringify(draft.inputPrompt) !== JSON.stringify(prompt);
  draft.inputStatus = status;
  draft.inputPrompt = prompt;
  if (draft.inputAnswer && (typeof draft.inputAnswer === 'string'
      ? draft.inputAnswer !== prompt?.id
      : draft.inputAnswer.id !== prompt?.id
        || (draft.inputAnswer.sent && draft.inputAnswer.revision !== screenMenuRevision(prompt))))
    draft.inputAnswer = null;
  if (changed && composerDraftOwner(composerUid) === owner) renderComposerInputStatus();
  bridge.status().paintTurn(uid);
}

async function probeComposerInput(uid) {
  const draft = composerDrafts.get(composerDraftOwner(uid)), name = bridge.terminal().takenOver(uid);
  if (!draft || !name) return {error:'会话尚未就绪，请切换终端检查'};
  const probe = (draft.inputProbe || 0) + 1;
  draft.inputProbe = probe;
  let data;
  try {
    data = await bridge.post().post('api/session/conversation/check', {uid, name,
      lease:bridge.terminal().termSendLease(name).lease || null}, {timeoutMs:5000});
  } catch (error) {
    data = {error:error.name === 'TimeoutError'
      ? 'CLI 输入状态检查超时，正在重试；可切换终端检查'
      : error.message || String(error)};
  }
  // An older poll cannot overwrite a newer SEND check, a switched view, or
  // the state of a replacement terminal using the same logical draft.
  if (draft.inputProbe === probe && bridge.terminal().takenOver(uid) === name) {
    updateComposerInputStatus(uid, data);
    if (data && typeof data === 'object' && data.cli) controller.applyCliState(uid, data.cli, {status:false});
  }
  return data;
}

function syncComposerSendState() {
  if(!composerSending) ui.busy='';
  bridge.pendingStage().renderQueuedSends(composerUid);
  const draft=composerDrafts.get(composerDraftOwner(composerUid));
  const blocked=composerUsesInputStatus() ? !composerInputAllowsSend(draft?.inputStatus) : !!activeCliQuestion(composerUid);
  ui.sendDisabled=!!bridge.build().state.stale || composerSending || !!draft?.loading || sessionComposerEnded(composerUid) || blocked;
}

function composerInputNotice(status) {
  if (!status) return '';
  const messages = {
    input_check_pending: '正在检查终端输入状态',
    cli_starting: '终端画面尚未就绪，正在重新检查；输入已保留',
    cli_catching_up: '终端画面正在同步，正在重新检查；输入已保留',
    cli_pasting: '检测到终端正在粘贴，正在重新检查；输入已保留',
    cli_question: '检测到终端选择界面，请切换到 PTY（终端）处理；输入已保留',
    cli_not_ready: '暂未识别到终端消息编辑区，请切换到 PTY（终端）查看；输入已保留',
  };
  // 终端优先的页面里终端已在上方，不再让用户"切换"过去。
  const above = {
    cli_question: '终端正在等待选择，请在上方终端处理；输入已保留',
    cli_not_ready: '暂未识别到终端消息编辑区，请先在上方终端关闭菜单或对话框；输入已保留',
    cli_input_pending: '终端输入框里已有未发送的文字，请在上方终端发送或清空；输入已保留',
    cli_input_returned: '上一条消息已被 Esc 退回终端输入框，请在上方终端按回车重发或清空；输入已保留',
  };
  if (typeof bridge.terminal().sessionTerminalFirst === 'function' && bridge.terminal().sessionTerminalFirst(composerUid)
      && above[status.code])
    return above[status.code];
  return messages[status.code] || status.message;
}

function renderComposerInputStatus() {
  const draft=composerDrafts.get(composerDraftOwner(composerUid));
  const status=draft && composerUsesInputStatus() && !composerInputAllowsSend(draft.inputStatus)
    ? draft.inputStatus || composerInputStatus(null) : null;
  ui.blocked=!!(status && (status.state==='blocked' || (status.state==='unknown' && status.code!=='input_check_pending')));
  ui.attention=status ? bridge.status().sessionInputAttention(composerUid) : '';
  ui.notice=status?.code==='cli_question' && draft?.inputPrompt
    ? '等待用户回答，请在题卡中选择；输入已保留' : composerInputNotice(status);
  renderComposerQuestion(draft); syncComposerSendState();
}

function renderComposerQuestion(draft) {
  const prompt=draft?.inputPrompt;
  const signature=JSON.stringify([composerUid,prompt,draft?.inputAnswer]);
  if(ui.signature===signature) return;
  ui.signature=signature;
  if(!prompt) {
    ui.question=null;
    if(draft?.inputStatus?.state==='ready' || sessionComposerEnded(composerUid)) {
      const prefix=`${composerDraftOwner(composerUid)}\0`;
      for(const key of bridge.questionRuntime().screenMenuTextDrafts.keys()) if(key.startsWith(prefix)) bridge.questionRuntime().screenMenuTextDrafts.delete(key);
    }
    return;
  }
  const m={...prompt,uid:composerUid,call_id:prompt.id,live:true,state:draft.inputAnswer ? 'submitted' : 'waiting'};
  const rows=Array.isArray(m.questions) && m.questions.length ? m.questions : [{question:m.text,options:[]}];
  const form=sessiondockCli(m.source || m.uid)?.canAnswerQuestionForm({...m,questions:rows});
  const draftKey=form && m.uid && m.call_id ? `${m.uid}\0${m.call_id}` : '';
  let selected=draftKey ? bridge.questionRuntime().questionFormDrafts.get(draftKey) : null;
  if(!Array.isArray(selected) || selected.length!==rows.length || !selected.every((index,i)=>index===null || (Number.isInteger(index)&&!!rows[i]?.options?.[index]))) {
    selected=Array(rows.length).fill(null); if(draftKey) bridge.questionRuntime().questionFormDrafts.set(draftKey,selected);
  }
  const key=`${composerDraftOwner(m.uid)}\0${m.id}`;
  if(m.text && typeof m.text==='object') {
    const prefix=`${composerDraftOwner(m.uid)}\0`;
    for(const saved of bridge.questionRuntime().screenMenuTextDrafts.keys()) if(saved.startsWith(prefix)&&saved!==key) bridge.questionRuntime().screenMenuTextDrafts.delete(saved);
  }
  ui.questionUi={selected,submitting:null,
    text:bridge.questionRuntime().screenMenuTextDrafts.get(key) ?? m.text?.value ?? ''};
  ui.question=m;
}

async function answerComposerQuestion(uid, id, index) {
  if (composerDraft(uid, false)?.inputPrompt?.kind === 'screen_menu')
    return answerComposerScreenMenu(uid, id, 'option', index);
  const draft = composerDraft(uid, false), name = bridge.terminal().takenOver(uid);
  if (!draft || draft.inputPrompt?.id !== id || draft.inputAnswer) return false;
  const binding = bridge.terminal().termInputBody(name, {name, keys:[], uid});
  if (!binding) return false;
  draft.inputAnswer = id;
  renderComposerQuestion(draft);
  // Recheck the same live menu before writing, including pre-rollout launches.
  const current = await controller.probeComposerInput(uid);
  if (current?.prompt?.id !== id || composerUid !== uid || bridge.terminal().takenOver(uid) !== name) {
    if (draft.inputAnswer === id) draft.inputAnswer = null;
    if (composerUid === uid) renderComposerQuestion(draft);
    return false;
  }
  const keys = sessiondockCli(current.prompt.source)?.questionAnswerKeys(current.prompt, index);
  let ok = false;
  try { ok = !!keys?.length && await writeComposerMenuInput(name, uid, binding, {keys}); }
  catch (error) { await SessionUi.appAlert('回答未完成，请检查终端后继续：' + (error.message || error)); }
  if (!ok) { draft.inputAnswer = null; if (composerUid === uid) renderComposerQuestion(draft); }
  else pollComposerInput();
  return !!ok;
}

async function writeComposerMenuInput(name, uid, binding, payload) {
  if (bridge.terminal().takenOver(uid) !== name || composerUid !== uid) return false;
  const response = await bridge.post().post('api/term/send', {...binding, ...payload});
  if (response.error) throw new Error(response.error);
  bridge.runtime().state.live.live.add(uid); bridge.runtime().state.live.liveTmux.add(uid); bridge.runtime().state.selection.lastSync = 0; bridge.runtime().state.selection.syncGap = 350;
  bridge.status().paintLive();
  return true;
}

async function answerComposerScreenMenu(uid, id, kind, value) {
  const draft = composerDraft(uid, false), shown = draft?.inputPrompt, name = bridge.terminal().takenOver(uid);
  if (!name || shown?.id !== id || draft.inputAnswer || composerUid !== uid) return false;
  const binding = bridge.terminal().termInputBody(name, {name, keys:[], uid});
  if (!binding) return false;
  delete binding.keys;
  const pending = {id, revision:screenMenuRevision(shown), sent:false};
  draft.inputAnswer = pending;
  renderComposerQuestion(draft);
  let ok = false;
  let refreshOnly = false;
  try {
    const current = await controller.probeComposerInput(uid), prompt = current?.prompt;
    if (prompt?.id !== id || composerUid !== uid || bridge.terminal().takenOver(uid) !== name
        || draft.inputAnswer !== pending) return false;
    let keys;
    if (kind === 'option') {
      const prior = shown.questions?.[0]?.options?.[value];
      const option = prompt.questions?.[0]?.options?.[value];
      if (!option || option.label !== prior?.label
          || (option.toggle && option.selected !== prior.selected)) return false;
      keys = option.keys;
    } else if (kind === 'action') {
      const item = prompt.actions?.[value];
      if (item?.label !== shown.actions?.[value]?.label) return false;
      keys = item?.keys;
      const navigation = new Set(['Up','Down','Left','Right','Home','End','Tab','BTab',
        'PageUp','PageDown','ctrl-n','ctrl-p','C-n','C-p']);
      refreshOnly = !!keys?.length && keys.every(key => navigation.has(key));
    } else if (kind === 'cancel') keys = prompt.cancel_keys;
    else if (kind === 'text') {
      if (!prompt.text || prompt.text.label !== shown.text?.label) return false;
      refreshOnly = !prompt.text.after_keys?.length;
      // Pin every write to the same instance. Native field text is separate
      // from the message draft, and a partial write is never retried.
      const write = payload => writeComposerMenuInput(name, uid, binding, payload);
      if (prompt.text.before_keys?.length && !await write({keys:prompt.text.before_keys})) return false;
      const payload = prompt.text.mode === 'data' ? {data:String(value)} : {paste:String(value)};
      if (String(value) && !await write(payload)) return false;
      if (String(value) && prompt.text.mode !== 'data') await new Promise(resolve => setTimeout(resolve, 600));
      if (prompt.text.after_keys?.length && !await write({keys:prompt.text.after_keys})) return false;
      ok = true;
    }
    if (kind !== 'text') ok = !!keys?.length && await writeComposerMenuInput(name, uid, binding, {keys});
    if (ok) {
      pending.sent = true;
      pending.revision = screenMenuRevision(prompt);
      bridge.runtime().state.selection.lastSync = 0;
      if (refreshOnly) {
        // Navigation at a boundary may legitimately leave the screen unchanged.
        // Reobserve once, then allow the next explicit operation; never resend.
        await new Promise(resolve => setTimeout(resolve, 200));
        await controller.probeComposerInput(uid);
        if (draft.inputAnswer === pending) {
          draft.inputAnswer = null;
          if (composerUid === uid) renderComposerQuestion(draft);
        }
      } else pollComposerInput();
    }
    return !!ok;
  } catch (error) {
    await SessionUi.appAlert('回答未完成，请检查终端后继续：' + (error.message || error));
    return false;
  } finally {
    if (!ok && draft.inputAnswer === pending) {
      draft.inputAnswer = null;
      if (composerUid === uid) renderComposerQuestion(draft);
    }
  }
}

async function reconcileComposerSubmission(uid) {
  const draft=composerDrafts.get(composerDraftOwner(uid));
  if (!draft?.requestId || draft.loading || draft.loadFailed || composerSending
      || composerSaving.has(draft) || draft.editVersion!==draft.savedVersion) return;
  const version=draft.editVersion,id=draft.requestId;
  const result=await priorComposerSubmission(uid,id);
  if (result?.state!=='sent' || !result.draft || result.draft.revision<draft.revision
      || draft.editVersion!==version || composerSaving.has(draft) || composerSending) return;
  // A later attachment may be saved as metadata while its bytes still live in
  // this page. A receipt refresh must preserve that File and its preview.
  adoptServerDraft(draft,result.draft,uid);
  refreshComposerDraft(composerDraftOwner(uid));syncComposerUnloadProtection();
}

let composerInputProbeBusy=false, composerDraftSyncBusy=false;

async function pollComposerInput() {
  if (bridge.runtime().network.paused) return;
  const uid=composerUid;
  if (!uid || !conversationSendEnabled() || document.hidden || composerSending || composerInputProbeBusy
      || !$('#composer').getClientRects().length || !bridge.terminal().takenOver(uid)) return;
  composerInputProbeBusy=true;
  let data;
  try { data = await controller.probeComposerInput(uid); }
  catch { /* SEND independently checks the current input surface. */ }
  finally { composerInputProbeBusy=false; }
  // Draft/history reads must not hold up the live input-status checks.
  if (composerDraftSyncBusy) return;
  composerDraftSyncBusy=true;
  try {
    if (Number.isInteger(data?.draft_revision)) await followServerDraft(uid,data.draft_revision);
    await reconcileComposerSubmission(uid);
  } catch { /* A missing/in-progress receipt keeps the editor intact. */ }
  finally { composerDraftSyncBusy=false; }
}

function scheduleComposerInputChecks(poll) {
  setInterval(poll, 1500);
  // Parent layout can hide the composer without changing its own classes.
  // Resume immediately on terminal/split toggles that reveal the input.
  let wasVisible = false;
  const visibility = new ResizeObserver(() => {
    const visible = !!$('#composer').getClientRects().length;
    const resumed = visible && !wasVisible;
    wasVisible = visible;
    if (resumed) poll();
  });
  visibility.observe($('#composer'));
}

function renderComposerItems() {
  const draft=composerDraft();
  ui.uid=composerUid; ui.text=draft?.text || ''; ui.loading=!!draft?.loading;
  ui.sending=composerSending; ui.addDisabled=!!bridge.build().state.stale || composerSending || !!draft?.loading;
  ui.attachments=(draft?.attachments || []).map(a=>({...a})); ui.quotes=(draft?.quotes || []).map(q=>({...q}));
  ui.storageError=draft?.storageError || '';
  ui.restartable=!!(composerUid?.startsWith('tmux:') && !bridge.terminal().takenOver(composerUid)
    && ['exited','failed'].includes((bridge.terminal().state.pending || []).find(r=>pendingUid(r.name)===composerUid)?.state)
    && !bridge.terminal().sessionIsPtyOnly(composerUid));
  for(const attachment of draft?.attachments || []) if(attachment.kind==='image') loadStagedComposerPreview(attachment,composerUid,renderComposerItems);
  renderComposerInputStatus();
}

function addComposerFiles(files) {
  const draft = composerDraft();
  if (!draft) return;
  const before = new Set(draft.attachments);
  addDraftFiles(draft, files);
  persistComposerDraft();
  for (const attachment of draft.attachments) {
    if (!before.has(attachment)) stageComposerAttachment(attachment, composerUid);
  }
  renderComposerItems();
}

function addDraftFiles(draft, files) {
  for (const file of files) {
    if (draft.attachments.length >= COMPOSER_MAX_FILES) {
      SessionUi.appAlert(`一次最多添加 ${COMPOSER_MAX_FILES} 个附件`);
      break;
    }
    if (!file.size || file.size > COMPOSER_MAX_FILE_BYTES) {
      SessionUi.appAlert(`「${file.name || '附件'}」为空或超过 512 MB`);
      continue;
    }
    const kind = composerFileKind(file);
    draft.attachments.push({
      id: `attachment-${globalThis.crypto?.randomUUID?.() || ++composerDraftSeq}`, number: draft.nextAttachmentNumber++, file, kind,
      preview: kind === 'image' ? URL.createObjectURL(file) : '',
      status: '', uploaded: null, error: '',
    });
  }
}

function insertComposerReference(number, ta = $('#cinput')) {
  if (!ta) return;
  const token = `[附件${number}]`;
  ta.focus();
  const start = Number.isInteger(ta.selectionStart) ? ta.selectionStart : ta.value.length;
  const end = Number.isInteger(ta.selectionEnd) ? ta.selectionEnd : start;
  ta.setRangeText(token, start, end, 'end');
  ta.dispatchEvent(new Event('input', { bubbles: true }));
}

function removeDraftAttachment(draft, id) {
  const at = draft.attachments.findIndex(x => x.id === id);
  if (at < 0) return null;
  const [removed] = draft.attachments.splice(at, 1);
  if (removed.preview) URL.revokeObjectURL(removed.preview);
  if (removed.cancelUpload) removed.cancelUpload();
  return removed;
}

function removeComposerAttachment(id, draft = composerDraft()) {
  if (!draft || composerSending) return;
  const removed = removeDraftAttachment(draft, id);
  discardStagedAttachment(removed, persistComposerDraft());
  renderComposerItems();
}

function addComposerQuote(text = '') {
  const draft = composerDraft();
  if (!draft) return;
  if (draft.quotes.length >= 4) return SessionUi.appAlert('一次最多添加 4 段引用');
  draft.quotes.push({ id: `quote-${globalThis.crypto?.randomUUID?.() || ++composerDraftSeq}`, text: String(text).trim().slice(0, 16000) });
  persistComposerDraft();
  renderComposerItems();
  boxFocusLastQuote();
}

function boxFocusLastQuote() {
  requestAnimationFrame(() => {
    const nodes = document.querySelectorAll('#compose-items .draft-quote textarea');
    nodes[nodes.length - 1]?.focus();
  });
}

function removeComposerQuote(id, draft = composerDraft()) {
  if (!draft || composerSending) return;
  const at = draft.quotes.findIndex(x => x.id === id);
  if (at >= 0) draft.quotes.splice(at, 1);
  persistComposerDraft();
  renderComposerItems();
}

async function uploadComposerAttachment(attachment, uid, attachmentId = null,
  {node = '', render = renderComposerItems} = {}) {
  if (attachment.uploaded?.upload_id && composerDraftOwner(attachment.uploaded.uid) === composerDraftOwner(uid)
      && (!node || attachment.uploaded.node === node || bridge.runtime().nodes.nodeOf(attachment.uploaded.uid) === node)) return attachment.uploaded;
  if (!(attachment.file instanceof Blob)) throw new Error('请重新选择未上传的附件：' + attachment.file.name);
  if (attachment.file.size > COMPOSER_MAX_FILE_BYTES) throw new Error('单个附件不能超过 512 MiB');
  attachment.status = 'uploading'; attachment.error = ''; attachment.progress = 0; render();
  const url = new URL(bridge.environment.appUrl('api/session/conversation/attachment'));
  url.searchParams.set('uid', uid); url.searchParams.set('id', attachment.id);
  url.searchParams.set('name', attachment.file.name || 'attachment');
  if (node) url.searchParams.set('node', node);
  try {
    const sendUpload = () => new Promise((resolve, reject) => {
      const xhr = new XMLHttpRequest();
      attachment.cancelUpload = () => xhr.abort();
      xhr.open('POST', url);
      xhr.setRequestHeader('Content-Type', attachment.file.type || 'application/octet-stream');
      xhr.upload.onprogress = event => {
        if (event.lengthComputable) attachment.progress = Math.floor(event.loaded / event.total * 100);
        render();
      };
      xhr.onload = () => {
        let data;
        const failure = message => Object.assign(new Error(message), {
          retryUpload: [502, 503, 504].includes(xhr.status),
        });
        try {data = JSON.parse(xhr.responseText);} catch {return reject(failure(`HTTP ${xhr.status}`));}
        if (xhr.status < 200 || xhr.status >= 300 || data.error) reject(failure(data.error || `HTTP ${xhr.status}`));
        else resolve(data);
      };
      xhr.onerror = () => reject(Object.assign(new Error('上传连接中断'), {retryUpload: true}));
      xhr.onabort = () => reject(new Error('上传已取消'));
      xhr.send(attachment.file);
    });
    let data;
    try {data = await sendUpload();}
    catch (error) {
      if (!error.retryUpload) throw error;
      // Staging is idempotent by (draft uid, upload id, bytes). A lost reply
      // can safely repeat this upload, including after the node saved it.
      // This never retries the report launch or the conversation SEND.
      data = await sendUpload();
    }
    // The request has finished. Keeping its abort handler while the draft is
    // being saved makes the ready card's remove button abort a completed XHR.
    delete attachment.cancelUpload;
    attachment.uploaded = {...data, uid, node}; attachment.status = 'ready'; render();
    // Uploaded references become durable before any publication or SEND.
    if (!await persistComposerDraft(uid)) throw new Error('附件已上传，草稿引用保存失败');
    return attachment.uploaded;
  } catch (error) {
    attachment.status = 'failed'; attachment.error = error.message || String(error); render(); throw error;
  } finally {delete attachment.cancelUpload;}
}

const COMPOSER_UPLOAD_LANES = 2;

const composerUploadLanes = new Map();

function stageComposerAttachment(attachment, uid, options = {}) {
  const draft = composerDrafts.get(composerDraftOwner(uid));
  if (!draft || !conversationSendEnabled()) return Promise.resolve(null);
  if (attachment.staging) return attachment.staging;
  if (attachment.uploaded?.upload_id || !(attachment.file instanceof Blob)) return Promise.resolve(attachment.uploaded);
  const lane = composerUploadLanes.get(draft) || composerUploadLanes.set(draft, {queue: [], active: 0}).get(draft);
  const render = options.render || renderComposerItems;
  attachment.status = 'queued'; attachment.error = ''; render();
  attachment.staging = new Promise(resolve => lane.queue.push(async () => {
    try {
      if (!draft.attachments.includes(attachment)) { attachment.status = ''; resolve(null); return; }
      resolve(await uploadComposerAttachment(attachment, uid, null, options));
    } catch {
      resolve(null); // The failed card keeps the File; retry or SEND uploads again.
    } finally {
      delete attachment.staging;
    }
  }));
  pumpComposerUploads(lane);
  return attachment.staging;
}

function pumpComposerUploads(lane) {
  while (lane.active < COMPOSER_UPLOAD_LANES && lane.queue.length) {
    lane.active++;
    lane.queue.shift()().finally(() => { lane.active--; pumpComposerUploads(lane); });
  }
}

const COMPOSER_PREVIEW_MAX_BYTES = 32 * 1024 * 1024;

function loadStagedComposerPreview(attachment, uid, render = () => {}) {
  if (!uid || attachment.kind !== 'image' || attachment.preview || attachment.previewLoading
    || Date.now() < (attachment.previewRetryAt || 0)) return;
  const id = attachment.uploaded?.upload_id;
  const size = attachment.file?.size ?? attachment.uploaded?.size ?? 0;
  if (!id || attachment.file instanceof Blob || size > COMPOSER_PREVIEW_MAX_BYTES) return;
  const url = new URL(bridge.environment.appUrl('api/session/conversation/attachment'));
  url.searchParams.set('uid', uid); url.searchParams.set('id', id);
  attachment.previewLoading = true;
  (async () => {
    try {
      const response = await bridge.runtime().network.fetch(url, {cache:'no-store'});
      // Bytes that are gone stay gone; a transient failure may be retried.
      if (response.status === 404) attachment.previewRetryAt = Infinity;
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const blob = await response.blob();
      const live = composerDrafts.get(composerDraftOwner(uid))?.attachments.includes(attachment);
      // The card may be gone by now: never leak the object URL of a dropped one.
      if (live && !attachment.preview) attachment.preview = URL.createObjectURL(blob);
    } catch {
      attachment.previewRetryAt ||= Date.now() + 60000;
    } finally {
      delete attachment.previewLoading;
      render();
    }
  })();
}

function discardStagedAttachment(attachment, saved = Promise.resolve(true)) {
  const id = attachment?.uploaded?.upload_id, uid = attachment?.uploaded?.uid;
  if (!id || !uid || !conversationSendEnabled()) return;
  Promise.resolve(saved)
    .then(ok => ok && bridge.post().post('api/session/conversation/attachment/discard', {uid, id}))
    .catch(() => {});
}

let composerSending = false;

async function submitComposer() {
  if (typeof bridge.build().state.stale !== 'undefined' && bridge.build().state.stale) return;
  if (!conversationSendEnabled()
      && !(typeof bridge.terminal().sessionIsPtyOnly === 'function' && bridge.terminal().sessionIsPtyOnly(composerUid || bridge.runtime().state.selection.sel))) {
    await SessionUi.appAlert('此服务尚未启用会话发送，请更新服务后重试'); return;
  }
  const ta = $('#cinput'), button = $('#csend');
  const uid = composerUid;
  let draft = composerDraft(uid);
  if (!draft || !bridge.terminal().takenOver(uid)) {
    await SessionUi.appAlert('会话尚未就绪，输入已保留；可切换到终端检查启动状态'); return;
  }
  const text = ta.value, attachments = [...draft.attachments];
  const quotes = draft.quotes.map(x => ({id:x.id, text:x.text})).filter(x => x.text.trim());
  if (composerSending || (!text.trim() && !attachments.length && !quotes.length)) return;
  // Enter obeys the same readiness gate as the Send button. The reason stays
  // beside the editor and polling clears it when the CLI becomes ready.
  if (composerUsesInputStatus(uid) && !composerInputAllowsSend(draft.inputStatus)) {
    renderComposerInputStatus();
    return;
  }
  if (!conversationSendEnabled() || (typeof bridge.terminal().sessionIsPtyOnly === 'function' && bridge.terminal().sessionIsPtyOnly(uid))) {
    closeComposerHistory(); composerSending = true;
    ui.sendDisabled=true;
    try {
      const sent = await sendToSession(text, null, uid);
      if (sent) {
        ta.value = '';
        draft.text = '';
        // The typed text was saved while typing; clear the server copy too, or
        // the exited console would come back as a "retained draft" row.
        persistComposerDraft(uid);
      }
    } catch (error) {
      await SessionUi.appAlert('发送失败，输入保留：' + (error.message || error));
    } finally {
      composerSending = false; ui.sendDisabled=false;renderComposerItems(); autoGrow(ta);
    }
    return;
  }
  closeComposerHistory(); composerSending = true;
  let sendFailed = false;
  ui.sendDisabled=true; ui.addDisabled=true;
  setComposerBusy( '发送中');
  renderComposerItems();
  try {
    draft.text = text;
    const priorPayload=JSON.stringify({text,attachments:attachments.map(a=>({upload_id:a.uploaded?.upload_id,number:a.number})),quotes});
    if (draft.requestId && (draft.requestText===priorPayload || (draft.report_prompt && draft.report_text===text))) {
      const previous=await priorComposerSubmission(uid,draft.requestId);
      if (previous?.state==='sent') {
        acceptComposerServerRevision(draft,previous.draft);
        await consumeComposerSubmission(uid,text,attachments,quotes);return;
      }
    }
    // Persist the payload and its stable submission ID together below.
    // SEND performs fresh readiness checks before publishing, pasting and Enter;
    // an extra browser CHECK only adds another serial network/identity lookup.
    const uploaded = [];
    for (let i = 0; i < attachments.length; i++) {
      setComposerBusy( `上传 ${i + 1}/${attachments.length}`);
      if (attachments[i].staging) await attachments[i].staging; // Staged on add; only a failure uploads here.
      uploaded.push({...await uploadComposerAttachment(attachments[i], uid), number:attachments[i].number});
    }
    const requestText = JSON.stringify({text, attachments:uploaded.map(a => ({upload_id:a.upload_id,number:a.number})), quotes});
    if (!(draft.report_prompt && draft.report_text===text && draft.requestId)
        && (draft.requestText !== requestText || !draft.requestId)) {
      draft.requestText = requestText; draft.requestId = crypto.randomUUID();
    }
    if (!await persistComposerDraft(uid)) throw new Error(draft.storageError || '提交标识尚未保存');
    const submittedRevision = draft.revision;
    setComposerBusy( '发送中');
    const sent = await sendToSession(text, null, uid, [], {requestId:draft.requestId,
      draftRevision:submittedRevision, attachments:uploaded, quotes});
    if (sent) await consumeComposerSubmission(uid,text,attachments,quotes);
    else sendFailed = true;
  } catch (error) {
    sendFailed = true;
    await SessionUi.appAlert('发送失败，输入保留：' + (error.message || error));
  } finally {
    composerSending = false; setComposerBusy( ''); ui.addDisabled=false;
    renderComposerItems(); autoGrow(ta);
    // A disabled Send button (including keyboard/touch activation) can leave
    // focus on <body> after the error dialog closes. Resume the retained draft
    // only if the user has not focused another control meanwhile.
    if (sendFailed && composerUid === uid && !ta.disabled
        && (document.activeElement === button || document.activeElement === document.body)) {
      ta.focus({preventScroll:true});
    }
  }
}

let composerEscAt = -Infinity;

async function revealNativeTerminal(uid = bridge.runtime().state.selection.sel) {
  const name = bridge.terminal().takenOver(uid);
  if (!name || bridge.runtime().state.selection.sel !== uid) return false;
  bridge.terminal().state.uid = uid;
  await bridge.terminal().openTermPane(name, true, bridge.environment.MOBILE.matches ? null : 'full');
  return true;
}

function activeCliQuestion(uid) {
  const entry = bridge.runtime().cache.cache.get(viewKey(uid));
  if (entry?.prompt?.questions?.length) return entry.prompt;
  const question = typeof bridge.questionRuntime().pendingHistoryQuestion === 'function'
    ? bridge.questionRuntime().pendingHistoryQuestion(entry) : null;
  return question ? {id: question.call_id, questions: question.questions} : null;
}

async function answerCliQuestion(uid, optionIndex) {
  const prompt = activeCliQuestion(uid);
  const rows = prompt?.questions;
  if (rows?.length !== 1 || rows[0].multiple || !rows[0].options?.[optionIndex]) return false;
  // 不同 CLI 的菜单定位语义不同（Claude 用方向键，Codex 用数字直选），
  // 具体按键必须由各自实现决定，不能在公共交互层猜测当前光标位置。
  const keys = sessiondockCli(uid)?.questionAnswerKeys(prompt, optionIndex);
  if (!keys?.length) return false;
  return sendToSession(null, keys, uid);
}

async function answerCliQuestionForm(uid, optionIndexes) {
  const prompt = activeCliQuestion(uid);
  const cli = sessiondockCli(uid);
  if (!cli?.canAnswerQuestionForm(prompt)) return false;
  const groups = cli.questionFormAnswerKeyGroups(prompt, optionIndexes);
  if (!groups?.length) return false;
  for (let i = 0; i < groups.length; i++) {
    if (!await sendToSession(null, groups[i], uid)) return false;
    // Enter 会让 Claude 卸载当前题并渲染下一题或 Review。分开发送并留出
    // 一个短事件循环间隔，避免后一题按键被旧题的输入处理器吞掉。
    if (i < groups.length - 1) {
      await new Promise(resolve => setTimeout(resolve, 50));
    }
  }
  return true;
}

async function cancelCliQuestion(uid) {
  composerEscAt = -Infinity;
  const keys = sessiondockCli(uid)?.questionCancelKeys(activeCliQuestion(uid));
  return keys?.length ? sendToSession(null, keys, uid) : false;
}

async function sendComposerEscape(now = performance.now()) {
  const uid = bridge.runtime().state.selection.sel;
  const entry = bridge.runtime().cache.cache.get(viewKey(uid));
  const visibleActivity = $('#activity')?.dataset.state;
  // activity 缓存可能来自上一个已结束的 Claude 进程；renderActivity 会把它
  // 隐藏。Esc 必须服从用户眼前的交互态，不能被这条旧 working 永久挡住回滚。
  const busy = !!entry?.prompt
    || !!bridge.questionRuntime().pendingHistoryQuestion(entry)
    || ['working', 'waiting'].includes(visibleActivity);
  const draft = composerDraft(uid, false);
  const empty = !String($('#cinput')?.value || '').trim()
    && !(draft?.attachments?.length) && !(draft?.quotes?.some(q => q.text?.trim()));
  const escape = sessiondockCli(uid)?.repeatedEscape(now, composerEscAt, { busy, empty })
    || { rewind: false, nextAt: -Infinity };
  const rewind = escape.rewind;
  composerEscAt = escape.nextAt;
  const name = bridge.terminal().takenOver(uid);
  const sent = await sendToSession(null, ['Escape'], uid);
  if (!rewind || !sent || !name || bridge.runtime().state.selection.sel !== uid) return sent;

  // 回滚点、恢复代码/对话的选项都由原生 CLI 自己维护。第二次 Esc 后直接
  // 揭示原生 TUI；确认回滚后服务端从编辑区与画面同步时间线（docs/cli-state.md）。
  await revealNativeTerminal(uid);
  return sent;
}

const PASTE_CONFIRM_FILES = 5;

const PASTE_CONFIRM_BYTES = 50 * 1024 * 1024;

function confirmPastedFiles(files) {
  const bytes = files.reduce((sum, file) => sum + (file.size || 0), 0);
  if (files.length <= PASTE_CONFIRM_FILES && bytes <= PASTE_CONFIRM_BYTES) return true;
  const size = bytes >= 1024 * 1024
    ? `${(bytes / 1048576).toFixed(bytes >= 100 * 1048576 ? 0 : 1)} MB`
    : `${Math.ceil(bytes / 1024)} KB`;
  return SessionUi.appConfirm(`粘贴了 ${files.length} 个文件，共 ${size}。继续？`);
}

function whenPasteConfirmed(files, go, then = () => {}) {
  const ok = confirmPastedFiles(files);
  if (ok === true) { go(); then(); }
  else ok.then(yes => { if (yes) go(); then(); });
}

function pasteAttachmentFiles(e, addFiles) {
  const directories = clipboardDirectoryNames(e.clipboardData);
  const files = clipboardAttachmentFiles(e.clipboardData);
  if (directories.length) {
    e.preventDefault();
    const warn = () => SessionUi.appAlert(`暂不支持直接粘贴文件夹：${directories.join('、')}。请先压缩后再粘贴。`);
    if (files.length) whenPasteConfirmed(files, () => addFiles(files), warn);
    else warn();
    return;
  }
  if (!files.length) {
    // 表格软件偶尔只提供 text/csv 剪贴板项而不提供 File。此时保留其
    // 二进制附件语义；普通 text/plain 粘贴仍完全交给浏览器。
    if (!clipboardCsvFile(e.clipboardData, file => addFiles([file]))) return;
    e.preventDefault();
    return;
  }
  // 带附件的剪贴板常同时携带 text/plain；交给浏览器会把那份文字再粘贴一次。
  e.preventDefault();
  whenPasteConfirmed(files, () => addFiles(files));
}

const $=(selector)=>document.querySelector(selector);
function setComposerBusy(label) {ui.busy=label;}
function hide() {ui.visible=false;}
function measureEditor() {
  autoGrow($('#cinput'));
  if(bridge.terminal().sessionTerminalFirst(composerUid) && typeof bridge.terminal().layoutTermPane==='function') bridge.terminal().layoutTermPane();
}
function input(e) {
  const draft = composerDraft();
  if (draft) { draft.text = e.target.value; persistComposerDraft(); }
  if (composerHistoryPicker.open && e.target.value !== '') closeComposerHistory();
  autoGrow(e.target);
ui.text=e.target.value;
}
function keydown(e) {
  if (e.isComposing) return;
  if (composerHistoryPicker.open) {
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      setComposerHistoryIndex(composerHistoryPicker.index - 1);
      return;
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setComposerHistoryIndex(composerHistoryPicker.index + 1);
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      acceptComposerHistory();
      return;
    }
    if (e.key === 'Escape') {
      e.preventDefault();
      closeComposerHistory();
      return;
    }
    if (!['Shift', 'Control', 'Alt', 'Meta'].includes(e.key)) closeComposerHistory();
  } else if (e.key === 'ArrowUp' && e.currentTarget.value === '') {
    e.preventDefault();
    openComposerHistory();
    return;
  }
  // 手机软键盘没有方便的 Shift+Enter：Enter 始终换行，只允许按钮发送。
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing && !bridge.environment.MOBILE.matches) {
    e.preventDefault();
    submitComposer();
  }
}
function sendMouseDown(event) {if(document.activeElement===$('#cinput')) event.preventDefault();}
function toggleAttachments(e) {e.stopPropagation();closeComposerHistory();ui.menu=!ui.menu;}
function chooseAttachment(type) {
  closeAttachMenu();
  if(type==='quote') {addComposerQuote(lastMessageSelectionUid===composerUid ? lastMessageSelection : '');lastMessageSelection='';lastMessageSelectionUid=null;return;}
  chooseAttachmentFiles(type,$('#cfile'),addComposerFiles);
}
function filesChanged(e) {addComposerFiles([...e.target.files]);e.target.value='';}
function paste(e) {pasteAttachmentFiles(e,addComposerFiles);}
function dragenter(e) {if(e.dataTransfer?.types?.includes('Files')) ui.dragover=true;}
function dragover(e) {if(!e.dataTransfer?.types?.includes('Files')) return;e.preventDefault();e.dataTransfer.dropEffect='copy';}
function dragleave(e) {if(!e.currentTarget.contains(e.relatedTarget)) ui.dragover=false;}
function drop(e) {ui.dragover=false;const files=[...(e.dataTransfer?.files || [])];if(!files.length)return;e.preventDefault();addComposerFiles(files);}
function quoteInput(id,e) {
  const q=composerDraft()?.quotes.find(q=>q.id===id);
  if(q){
    q.text=e.target.value;
    ui.quotes.find(quote=>quote.id===id).text=q.text;
    persistComposerDraft();
  }
}
function attachmentInsert(number) {insertComposerReference(number);}
function attachmentRemove(id) {const a=composerDraft()?.attachments.find(a=>a.id===id);if(a?.cancelUpload)a.cancelUpload();else removeComposerAttachment(id);}
function attachmentRetry(id) {const a=composerDraft()?.attachments.find(a=>a.id===id);if(a)stageComposerAttachment(a,composerUid);}
function questionText(e) {
  ui.questionUi={...ui.questionUi,text:e.target.value};
  bridge.questionRuntime().screenMenuTextDrafts.set(`${composerDraftOwner(composerUid)}\0${ui.question.id}`,e.target.value);
}
async function questionOption(i,j) {
  const m=ui.question;
  if(m.kind==='screen_menu') return answerComposerQuestion(m.uid,m.id,j);
  const rows=Array.isArray(m.questions) && m.questions.length ? m.questions : [{question:m.text,options:[]}];
  const form=(m.state || 'waiting')==='waiting' && sessiondockCli(m.source || m.uid)?.canAnswerQuestionForm({...m,questions:rows});
  if(form) {ui.questionUi.selected[i]=j;ui.questionUi={...ui.questionUi};return;}
  const pending={...ui.questionUi,submitting:j};ui.questionUi=pending;
  const ok=await answerComposerQuestion(m.uid,m.id,j);
  if(!ok && ui.questionUi===pending) ui.questionUi={...pending,submitting:null};
}
async function questionSubmit() {
  const m=ui.question,pending={...ui.questionUi,submitting:-1};ui.questionUi=pending;
  const ok=await answerCliQuestionForm(m.uid,pending.selected);
  if(!ok && ui.questionUi===pending) ui.questionUi={...pending,submitting:null};
}
function questionCancel() {const m=ui.question;return m.kind==='screen_menu' ? answerComposerScreenMenu(m.uid,m.id,'cancel') : answerComposerQuestion(m.uid,m.id,1);}
function questionAction(index) {const m=ui.question;return answerComposerScreenMenu(m.uid,m.id,'action',index);}
function questionTextSubmit() {const m=ui.question;return answerComposerScreenMenu(m.uid,m.id,'text',ui.questionUi.text);}
function questionTerminal() {return revealNativeTerminal(ui.question.uid);}
function cliInfo(m) {return sessiondockCli(m.source || m.uid);}
function attachmentCanRetry(a) {return a.file instanceof Blob;}
function attachmentMeta(a) {return fmtSize(a.file?.size ?? a.uploaded?.size ?? 0);}
function historyTime(ts,index) {return ts ? fmtTime(ts) : `${index+1}`;}
function start() {
  setInterval(recoverComposerDrafts,3000);addEventListener('online',recoverComposerDrafts);
  scheduleComposerInputChecks(pollComposerInput);
  bridge.environment.MOBILE.addEventListener('change',()=>{syncComposerMode();bridge.takeover().renderTakeoverBtn();});
  syncComposerMode();
  document.addEventListener('click',e=>{if(!e.target.closest('.attach-picker'))closeAttachMenu();if(!e.target.closest('.composer-input-wrap'))closeComposerHistory();});
  document.addEventListener('selectionchange',()=>{
    const selection=getSelection();if(!selection||selection.isCollapsed||!selection.anchorNode||!selection.focusNode)return;
    const messages=$('#msgs');if(messages?.contains(selection.anchorNode)&&messages.contains(selection.focusNode)) {
      lastMessageSelection=selection.toString().trim().slice(0,16000);lastMessageSelectionUid=bridge.runtime().state.selection.sel;
    }
  });
}
async function restartComposer() {
  const draft=composerDraft();
  if(!draft) return;
      ui.restarting=true;
      try {
        draft.restartId ||= crypto.randomUUID();
        const uid=composerUid;
        if (!await persistComposerDraft(uid)) throw new Error(draft.storageError || '草稿尚未保存');
        const data=await bridge.post().post('api/session/conversation/restart',{uid,request_id:draft.restartId});
        if (data.error) throw new Error(data.error);
        const next=pendingUid(data.name);
        // The server has already bound both instances to the same logical draft.
        migrateComposerDraft(uid,next);composerHydrations.delete(next);
        // Re-enter the editor for the replacement instance; the old CLI's
        // readiness and in-flight probes cannot authorize a send here.
        composerUid=null;
        await bridge.terminal().loadTermList();await bridge.terminal().openPendingSession(data);
      } catch (error){draft.storageError=error.message || String(error);renderComposerItems();}
  ui.restarting=false;
}

const controller = {
measureEditor,
attachmentCanRetry,
hide,
input,
keydown,
sendMouseDown,
toggleAttachments,
chooseAttachment,
filesChanged,
paste,
dragenter,
dragover,
dragleave,
drop,
quoteInput,
attachmentInsert,
attachmentRemove,
attachmentRetry,
questionText,
questionOption,
questionSubmit,
questionCancel,
questionAction,
questionTextSubmit,
questionTerminal,
cliInfo,
attachmentMeta,
historyTime,
start,
restartComposer,
chooseAttachmentFiles,
composerDraftOwner,
restoreComposerDraftRecord,
composerDraft,
composerDraftRecord,
priorComposerSubmission,
acceptComposerServerRevision,
applyCliState,
dismissQueuedSend,
consumeComposerSubmission,
readServerComposerDraft,
oldComposerDatabase,
importLegacyComposer,
hydrateComposerDraft,
mergeEarlyComposerEdit,
adoptServerDraft,
followServerDraft,
refreshComposerDraft,
queueComposerSave,
persistComposerDraft,
recoverComposerDrafts,
syncComposerUnloadProtection,
prepareComposerReload,
renderSavedComposerInputs,
recoverServerComposerDrafts,
recoverServerComposerNode,
deleteComposerDraftStorage,
pendingStartedAt,
rememberComposerSession,
syncComposerDraftBindings,
composerHistoryStamp,
nativeComposerHistory,
composerHistoryItems,
closeComposerHistory,
setComposerHistoryIndex,
renderComposerHistory,
openComposerHistory,
acceptComposerHistory,
ensureComposerAttachmentNumbers,
migrateComposerDraft,
switchComposerDraft,
sessionComposerEnded,
renderComposer,
autoGrow,
syncComposerMode,
sendToSession,
composerFileKind,
composerKindIcon,
closeAttachMenu,
renderAttachmentCards,
composerInputStatus,
composerInputAllowsSend,
composerUsesInputStatus,
updateComposerInputStatus,
probeComposerInput,
syncComposerSendState,
composerInputNotice,
renderComposerInputStatus,
renderComposerQuestion,
answerComposerQuestion,
screenMenuRevision,
writeComposerMenuInput,
answerComposerScreenMenu,
reconcileComposerSubmission,
pollComposerInput,
scheduleComposerInputChecks,
renderComposerItems,
addComposerFiles,
addDraftFiles,
clipboardAttachmentFiles,
clipboardDirectoryNames,
clipboardCsvFile,
insertComposerReference,
removeDraftAttachment,
removeComposerAttachment,
addComposerQuote,
boxFocusLastQuote,
removeComposerQuote,
uploadComposerAttachment,
stageComposerAttachment,
pumpComposerUploads,
loadStagedComposerPreview,
discardStagedAttachment,
submitComposer,
revealNativeTerminal,
activeCliQuestion,
answerCliQuestion,
answerCliQuestionForm,
cancelCliQuestion,
sendComposerEscape,
confirmPastedFiles,
whenPasteConfirmed,
pasteAttachmentFiles,
get COMPOSER_MAX_FILES(){return COMPOSER_MAX_FILES},
get COMPOSER_MAX_FILE_BYTES(){return COMPOSER_MAX_FILE_BYTES},
get ATTACH_ACCEPT(){return ATTACH_ACCEPT},
get composerDrafts(){return composerDrafts},
get composerDraftAliases(){return composerDraftAliases},
get composerInputHistoryCache(){return composerInputHistoryCache},
get composerHistoryPicker(){return composerHistoryPicker},
get composerUid(){return composerUid}, set composerUid(value){composerUid=value},
get composerDraftSeq(){return composerDraftSeq}, set composerDraftSeq(value){composerDraftSeq=value},
get lastMessageSelection(){return lastMessageSelection}, set lastMessageSelection(value){lastMessageSelection=value},
get lastMessageSelectionUid(){return lastMessageSelectionUid}, set lastMessageSelectionUid(value){lastMessageSelectionUid=value},
get conversationSendEnabled(){return conversationSendEnabled},
get newComposerDraft(){return newComposerDraft},
get composerHydrations(){return composerHydrations},
get composerDraftWrites(){return composerDraftWrites}, set composerDraftWrites(value){composerDraftWrites=value},
get composerSaveQueues(){return composerSaveQueues},
get legacyComposerDatabase(){return legacyComposerDatabase}, set legacyComposerDatabase(value){legacyComposerDatabase=value},
get composerFollowBusy(){return composerFollowBusy}, set composerFollowBusy(value){composerFollowBusy=value},
get composerFollowedAt(){return composerFollowedAt}, set composerFollowedAt(value){composerFollowedAt=value},
get composerPendingSaves(){return composerPendingSaves},
get composerSaving(){return composerSaving},
get composerRecoveryBusy(){return composerRecoveryBusy}, set composerRecoveryBusy(value){composerRecoveryBusy=value},
get composerUnloadWarning(){return composerUnloadWarning},
get composerUnloadProtected(){return composerUnloadProtected}, set composerUnloadProtected(value){composerUnloadProtected=value},
get composerServerRecoveries(){return composerServerRecoveries},
get composerInputProbeBusy(){return composerInputProbeBusy}, set composerInputProbeBusy(value){composerInputProbeBusy=value},
get composerDraftSyncBusy(){return composerDraftSyncBusy}, set composerDraftSyncBusy(value){composerDraftSyncBusy=value},
get COMPOSER_UPLOAD_LANES(){return COMPOSER_UPLOAD_LANES},
get composerUploadLanes(){return composerUploadLanes},
get COMPOSER_PREVIEW_MAX_BYTES(){return COMPOSER_PREVIEW_MAX_BYTES},
get composerSending(){return composerSending}, set composerSending(value){composerSending=value},
get composerEscAt(){return composerEscAt}, set composerEscAt(value){composerEscAt=value},
get PASTE_CONFIRM_FILES(){return PASTE_CONFIRM_FILES},
get PASTE_CONFIRM_BYTES(){return PASTE_CONFIRM_BYTES}
};
return controller;
}
