'use strict';
const composerController = SessionDockComposer.createController({
get HUB_MODE(){return typeof HUB_MODE === 'undefined' ? undefined : HUB_MODE},
get Nodes(){return typeof Nodes === 'undefined' ? undefined : Nodes},
get $(){return typeof $ === 'undefined' ? undefined : $},
get BUG_REPORT_DRAFT_UID(){return typeof BUG_REPORT_DRAFT_UID === 'undefined' ? undefined : BUG_REPORT_DRAFT_UID},
get BUILD_ID(){return typeof BUILD_ID === 'undefined' ? undefined : BUILD_ID},
get FAST_MIN(){return typeof FAST_MIN === 'undefined' ? undefined : FAST_MIN},
get MOBILE(){return typeof MOBILE === 'undefined' ? undefined : MOBILE},
get S(){return typeof S === 'undefined' ? undefined : S},
get STORAGE_PREFIX(){return typeof STORAGE_PREFIX === 'undefined' ? undefined : STORAGE_PREFIX},
get SessionDockCapabilities(){return typeof SessionDockCapabilities === 'undefined' ? undefined : SessionDockCapabilities},
get SessionDockNetwork(){return typeof SessionDockNetwork === 'undefined' ? undefined : SessionDockNetwork},
get T(){return typeof T === 'undefined' ? undefined : T},
get TERM_PAGE_ID(){return typeof TERM_PAGE_ID === 'undefined' ? undefined : TERM_PAGE_ID},
get appAlert(){return typeof appAlert === 'undefined' ? undefined : appAlert},
get appConfirm(){return typeof appConfirm === 'undefined' ? undefined : appConfirm},
get appUrl(){return typeof appUrl === 'undefined' ? undefined : appUrl},
get browserAuditEvent(){return typeof browserAuditEvent === 'undefined' ? undefined : browserAuditEvent},
get bugReportSending(){return typeof bugReportSending === 'undefined' ? undefined : bugReportSending},
get cache(){return typeof cache === 'undefined' ? undefined : cache},
get el(){return typeof el === 'undefined' ? undefined : el},
get fmtSize(){return typeof fmtSize === 'undefined' ? undefined : fmtSize},
get fmtTime(){return typeof fmtTime === 'undefined' ? undefined : fmtTime},
get layoutTermPane(){return typeof layoutTermPane === 'undefined' ? undefined : layoutTermPane},
get loadTermList(){return typeof loadTermList === 'undefined' ? undefined : loadTermList},
get nodeOf(){return typeof nodeOf === 'undefined' ? undefined : nodeOf},
get noteBugReportDraftNode(){return typeof noteBugReportDraftNode === 'undefined' ? undefined : noteBugReportDraftNode},
get openPendingSession(){return typeof openPendingSession === 'undefined' ? undefined : openPendingSession},
get openTermPane(){return typeof openTermPane === 'undefined' ? undefined : openTermPane},
get paintLive(){return typeof paintLive === 'undefined' ? undefined : paintLive},
get paintTurn(){return typeof paintTurn === 'undefined' ? undefined : paintTurn},
get pendingHistoryQuestion(){return typeof pendingHistoryQuestion === 'undefined' ? undefined : pendingHistoryQuestion},
get pendingPhase(){return typeof pendingPhase === 'undefined' ? undefined : pendingPhase},
get pendingTmuxSessions(){return typeof pendingTmuxSessions === 'undefined' ? undefined : pendingTmuxSessions},
get pendingUid(){return typeof pendingUid === 'undefined' ? undefined : pendingUid},
get post(){return typeof post === 'undefined' ? undefined : post},
get questionFormDrafts(){return typeof questionFormDrafts === 'undefined' ? undefined : questionFormDrafts},
get renderBugReportItems(){return typeof renderBugReportItems === 'undefined' ? undefined : renderBugReportItems},
get renderQueuedSends(){return typeof renderQueuedSends === 'undefined' ? undefined : renderQueuedSends},
get renderSide(){return typeof renderSide === 'undefined' ? undefined : renderSide},
get renderTakeoverBtn(){return typeof renderTakeoverBtn === 'undefined' ? undefined : renderTakeoverBtn},
get screenMenuTextDrafts(){return typeof screenMenuTextDrafts === 'undefined' ? undefined : screenMenuTextDrafts},
get sessionInputAttention(){return typeof sessionInputAttention === 'undefined' ? undefined : sessionInputAttention},
get sessionIsPtyOnly(){return typeof sessionIsPtyOnly === 'undefined' ? undefined : sessionIsPtyOnly},
get sessionTerminalEnabled(){return typeof sessionTerminalEnabled === 'undefined' ? undefined : sessionTerminalEnabled},
get sessionTerminalFirst(){return typeof sessionTerminalFirst === 'undefined' ? undefined : sessionTerminalFirst},
get sessiondockCli(){return typeof sessiondockCli === 'undefined' ? undefined : sessiondockCli},
get staleBuildShown(){return typeof staleBuildShown === 'undefined' ? undefined : staleBuildShown},
get store(){return typeof store === 'undefined' ? undefined : store},
get takenOver(){return typeof takenOver === 'undefined' ? undefined : takenOver},
get termInputBody(){return typeof termInputBody === 'undefined' ? undefined : termInputBody},
get termSendLease(){return typeof termSendLease === 'undefined' ? undefined : termSendLease},
get viewKey(){return typeof viewKey === 'undefined' ? undefined : viewKey}
});
Object.defineProperties(globalThis,{
"chooseAttachmentFiles":{configurable:true,get:()=>composerController.chooseAttachmentFiles,set:value=>{composerController.chooseAttachmentFiles=value}},
"composerDraftOwner":{configurable:true,get:()=>composerController.composerDraftOwner,set:value=>{composerController.composerDraftOwner=value}},
"restoreComposerDraftRecord":{configurable:true,get:()=>composerController.restoreComposerDraftRecord,set:value=>{composerController.restoreComposerDraftRecord=value}},
"composerDraft":{configurable:true,get:()=>composerController.composerDraft,set:value=>{composerController.composerDraft=value}},
"composerDraftRecord":{configurable:true,get:()=>composerController.composerDraftRecord,set:value=>{composerController.composerDraftRecord=value}},
"priorComposerSubmission":{configurable:true,get:()=>composerController.priorComposerSubmission,set:value=>{composerController.priorComposerSubmission=value}},
"acceptComposerServerRevision":{configurable:true,get:()=>composerController.acceptComposerServerRevision,set:value=>{composerController.acceptComposerServerRevision=value}},
"applyCliState":{configurable:true,get:()=>composerController.applyCliState,set:value=>{composerController.applyCliState=value}},
"dismissQueuedSend":{configurable:true,get:()=>composerController.dismissQueuedSend,set:value=>{composerController.dismissQueuedSend=value}},
"consumeComposerSubmission":{configurable:true,get:()=>composerController.consumeComposerSubmission,set:value=>{composerController.consumeComposerSubmission=value}},
"readServerComposerDraft":{configurable:true,get:()=>composerController.readServerComposerDraft,set:value=>{composerController.readServerComposerDraft=value}},
"oldComposerDatabase":{configurable:true,get:()=>composerController.oldComposerDatabase,set:value=>{composerController.oldComposerDatabase=value}},
"importLegacyComposer":{configurable:true,get:()=>composerController.importLegacyComposer,set:value=>{composerController.importLegacyComposer=value}},
"hydrateComposerDraft":{configurable:true,get:()=>composerController.hydrateComposerDraft,set:value=>{composerController.hydrateComposerDraft=value}},
"mergeEarlyComposerEdit":{configurable:true,get:()=>composerController.mergeEarlyComposerEdit,set:value=>{composerController.mergeEarlyComposerEdit=value}},
"adoptServerDraft":{configurable:true,get:()=>composerController.adoptServerDraft,set:value=>{composerController.adoptServerDraft=value}},
"followServerDraft":{configurable:true,get:()=>composerController.followServerDraft,set:value=>{composerController.followServerDraft=value}},
"refreshComposerDraft":{configurable:true,get:()=>composerController.refreshComposerDraft,set:value=>{composerController.refreshComposerDraft=value}},
"queueComposerSave":{configurable:true,get:()=>composerController.queueComposerSave,set:value=>{composerController.queueComposerSave=value}},
"persistComposerDraft":{configurable:true,get:()=>composerController.persistComposerDraft,set:value=>{composerController.persistComposerDraft=value}},
"recoverComposerDrafts":{configurable:true,get:()=>composerController.recoverComposerDrafts,set:value=>{composerController.recoverComposerDrafts=value}},
"syncComposerUnloadProtection":{configurable:true,get:()=>composerController.syncComposerUnloadProtection,set:value=>{composerController.syncComposerUnloadProtection=value}},
"prepareComposerReload":{configurable:true,get:()=>composerController.prepareComposerReload,set:value=>{composerController.prepareComposerReload=value}},
"renderSavedComposerInputs":{configurable:true,get:()=>composerController.renderSavedComposerInputs,set:value=>{composerController.renderSavedComposerInputs=value}},
"recoverServerComposerDrafts":{configurable:true,get:()=>composerController.recoverServerComposerDrafts,set:value=>{composerController.recoverServerComposerDrafts=value}},
"recoverServerComposerNode":{configurable:true,get:()=>composerController.recoverServerComposerNode,set:value=>{composerController.recoverServerComposerNode=value}},
"deleteComposerDraftStorage":{configurable:true,get:()=>composerController.deleteComposerDraftStorage,set:value=>{composerController.deleteComposerDraftStorage=value}},
"pendingStartedAt":{configurable:true,get:()=>composerController.pendingStartedAt,set:value=>{composerController.pendingStartedAt=value}},
"rememberComposerSession":{configurable:true,get:()=>composerController.rememberComposerSession,set:value=>{composerController.rememberComposerSession=value}},
"syncComposerDraftBindings":{configurable:true,get:()=>composerController.syncComposerDraftBindings,set:value=>{composerController.syncComposerDraftBindings=value}},
"composerHistoryStamp":{configurable:true,get:()=>composerController.composerHistoryStamp,set:value=>{composerController.composerHistoryStamp=value}},
"nativeComposerHistory":{configurable:true,get:()=>composerController.nativeComposerHistory,set:value=>{composerController.nativeComposerHistory=value}},
"composerHistoryItems":{configurable:true,get:()=>composerController.composerHistoryItems,set:value=>{composerController.composerHistoryItems=value}},
"closeComposerHistory":{configurable:true,get:()=>composerController.closeComposerHistory,set:value=>{composerController.closeComposerHistory=value}},
"setComposerHistoryIndex":{configurable:true,get:()=>composerController.setComposerHistoryIndex,set:value=>{composerController.setComposerHistoryIndex=value}},
"renderComposerHistory":{configurable:true,get:()=>composerController.renderComposerHistory,set:value=>{composerController.renderComposerHistory=value}},
"openComposerHistory":{configurable:true,get:()=>composerController.openComposerHistory,set:value=>{composerController.openComposerHistory=value}},
"acceptComposerHistory":{configurable:true,get:()=>composerController.acceptComposerHistory,set:value=>{composerController.acceptComposerHistory=value}},
"ensureComposerAttachmentNumbers":{configurable:true,get:()=>composerController.ensureComposerAttachmentNumbers,set:value=>{composerController.ensureComposerAttachmentNumbers=value}},
"migrateComposerDraft":{configurable:true,get:()=>composerController.migrateComposerDraft,set:value=>{composerController.migrateComposerDraft=value}},
"switchComposerDraft":{configurable:true,get:()=>composerController.switchComposerDraft,set:value=>{composerController.switchComposerDraft=value}},
"sessionComposerEnded":{configurable:true,get:()=>composerController.sessionComposerEnded,set:value=>{composerController.sessionComposerEnded=value}},
"renderComposer":{configurable:true,get:()=>composerController.renderComposer,set:value=>{composerController.renderComposer=value}},
"autoGrow":{configurable:true,get:()=>composerController.autoGrow,set:value=>{composerController.autoGrow=value}},
"syncComposerMode":{configurable:true,get:()=>composerController.syncComposerMode,set:value=>{composerController.syncComposerMode=value}},
"sendToSession":{configurable:true,get:()=>composerController.sendToSession,set:value=>{composerController.sendToSession=value}},
"composerFileKind":{configurable:true,get:()=>composerController.composerFileKind,set:value=>{composerController.composerFileKind=value}},
"composerKindIcon":{configurable:true,get:()=>composerController.composerKindIcon,set:value=>{composerController.composerKindIcon=value}},
"closeAttachMenu":{configurable:true,get:()=>composerController.closeAttachMenu,set:value=>{composerController.closeAttachMenu=value}},
"renderAttachmentCards":{configurable:true,get:()=>composerController.renderAttachmentCards,set:value=>{composerController.renderAttachmentCards=value}},
"composerInputStatus":{configurable:true,get:()=>composerController.composerInputStatus,set:value=>{composerController.composerInputStatus=value}},
"composerInputAllowsSend":{configurable:true,get:()=>composerController.composerInputAllowsSend,set:value=>{composerController.composerInputAllowsSend=value}},
"composerUsesInputStatus":{configurable:true,get:()=>composerController.composerUsesInputStatus,set:value=>{composerController.composerUsesInputStatus=value}},
"updateComposerInputStatus":{configurable:true,get:()=>composerController.updateComposerInputStatus,set:value=>{composerController.updateComposerInputStatus=value}},
"probeComposerInput":{configurable:true,get:()=>composerController.probeComposerInput,set:value=>{composerController.probeComposerInput=value}},
"syncComposerSendState":{configurable:true,get:()=>composerController.syncComposerSendState,set:value=>{composerController.syncComposerSendState=value}},
"composerInputNotice":{configurable:true,get:()=>composerController.composerInputNotice,set:value=>{composerController.composerInputNotice=value}},
"renderComposerInputStatus":{configurable:true,get:()=>composerController.renderComposerInputStatus,set:value=>{composerController.renderComposerInputStatus=value}},
"renderComposerQuestion":{configurable:true,get:()=>composerController.renderComposerQuestion,set:value=>{composerController.renderComposerQuestion=value}},
"answerComposerQuestion":{configurable:true,get:()=>composerController.answerComposerQuestion,set:value=>{composerController.answerComposerQuestion=value}},
"screenMenuRevision":{configurable:true,get:()=>composerController.screenMenuRevision,set:value=>{composerController.screenMenuRevision=value}},
"writeComposerMenuInput":{configurable:true,get:()=>composerController.writeComposerMenuInput,set:value=>{composerController.writeComposerMenuInput=value}},
"answerComposerScreenMenu":{configurable:true,get:()=>composerController.answerComposerScreenMenu,set:value=>{composerController.answerComposerScreenMenu=value}},
"reconcileComposerSubmission":{configurable:true,get:()=>composerController.reconcileComposerSubmission,set:value=>{composerController.reconcileComposerSubmission=value}},
"pollComposerInput":{configurable:true,get:()=>composerController.pollComposerInput,set:value=>{composerController.pollComposerInput=value}},
"scheduleComposerInputChecks":{configurable:true,get:()=>composerController.scheduleComposerInputChecks,set:value=>{composerController.scheduleComposerInputChecks=value}},
"renderComposerItems":{configurable:true,get:()=>composerController.renderComposerItems,set:value=>{composerController.renderComposerItems=value}},
"addComposerFiles":{configurable:true,get:()=>composerController.addComposerFiles,set:value=>{composerController.addComposerFiles=value}},
"addDraftFiles":{configurable:true,get:()=>composerController.addDraftFiles,set:value=>{composerController.addDraftFiles=value}},
"clipboardAttachmentFiles":{configurable:true,get:()=>composerController.clipboardAttachmentFiles,set:value=>{composerController.clipboardAttachmentFiles=value}},
"clipboardDirectoryNames":{configurable:true,get:()=>composerController.clipboardDirectoryNames,set:value=>{composerController.clipboardDirectoryNames=value}},
"clipboardCsvFile":{configurable:true,get:()=>composerController.clipboardCsvFile,set:value=>{composerController.clipboardCsvFile=value}},
"insertComposerReference":{configurable:true,get:()=>composerController.insertComposerReference,set:value=>{composerController.insertComposerReference=value}},
"removeDraftAttachment":{configurable:true,get:()=>composerController.removeDraftAttachment,set:value=>{composerController.removeDraftAttachment=value}},
"removeComposerAttachment":{configurable:true,get:()=>composerController.removeComposerAttachment,set:value=>{composerController.removeComposerAttachment=value}},
"addComposerQuote":{configurable:true,get:()=>composerController.addComposerQuote,set:value=>{composerController.addComposerQuote=value}},
"boxFocusLastQuote":{configurable:true,get:()=>composerController.boxFocusLastQuote,set:value=>{composerController.boxFocusLastQuote=value}},
"removeComposerQuote":{configurable:true,get:()=>composerController.removeComposerQuote,set:value=>{composerController.removeComposerQuote=value}},
"uploadComposerAttachment":{configurable:true,get:()=>composerController.uploadComposerAttachment,set:value=>{composerController.uploadComposerAttachment=value}},
"stageComposerAttachment":{configurable:true,get:()=>composerController.stageComposerAttachment,set:value=>{composerController.stageComposerAttachment=value}},
"pumpComposerUploads":{configurable:true,get:()=>composerController.pumpComposerUploads,set:value=>{composerController.pumpComposerUploads=value}},
"loadStagedComposerPreview":{configurable:true,get:()=>composerController.loadStagedComposerPreview,set:value=>{composerController.loadStagedComposerPreview=value}},
"discardStagedAttachment":{configurable:true,get:()=>composerController.discardStagedAttachment,set:value=>{composerController.discardStagedAttachment=value}},
"submitComposer":{configurable:true,get:()=>composerController.submitComposer,set:value=>{composerController.submitComposer=value}},
"revealNativeTerminal":{configurable:true,get:()=>composerController.revealNativeTerminal,set:value=>{composerController.revealNativeTerminal=value}},
"activeCliQuestion":{configurable:true,get:()=>composerController.activeCliQuestion,set:value=>{composerController.activeCliQuestion=value}},
"answerCliQuestion":{configurable:true,get:()=>composerController.answerCliQuestion,set:value=>{composerController.answerCliQuestion=value}},
"answerCliQuestionForm":{configurable:true,get:()=>composerController.answerCliQuestionForm,set:value=>{composerController.answerCliQuestionForm=value}},
"cancelCliQuestion":{configurable:true,get:()=>composerController.cancelCliQuestion,set:value=>{composerController.cancelCliQuestion=value}},
"sendComposerEscape":{configurable:true,get:()=>composerController.sendComposerEscape,set:value=>{composerController.sendComposerEscape=value}},
"confirmPastedFiles":{configurable:true,get:()=>composerController.confirmPastedFiles,set:value=>{composerController.confirmPastedFiles=value}},
"whenPasteConfirmed":{configurable:true,get:()=>composerController.whenPasteConfirmed,set:value=>{composerController.whenPasteConfirmed=value}},
"pasteAttachmentFiles":{configurable:true,get:()=>composerController.pasteAttachmentFiles,set:value=>{composerController.pasteAttachmentFiles=value}},
"COMPOSER_MAX_FILES":{configurable:true,get:()=>composerController.COMPOSER_MAX_FILES,set:value=>{composerController.COMPOSER_MAX_FILES=value}},
"COMPOSER_MAX_FILE_BYTES":{configurable:true,get:()=>composerController.COMPOSER_MAX_FILE_BYTES,set:value=>{composerController.COMPOSER_MAX_FILE_BYTES=value}},
"ATTACH_ACCEPT":{configurable:true,get:()=>composerController.ATTACH_ACCEPT,set:value=>{composerController.ATTACH_ACCEPT=value}},
"composerDrafts":{configurable:true,get:()=>composerController.composerDrafts,set:value=>{composerController.composerDrafts=value}},
"composerDraftAliases":{configurable:true,get:()=>composerController.composerDraftAliases,set:value=>{composerController.composerDraftAliases=value}},
"composerInputHistoryCache":{configurable:true,get:()=>composerController.composerInputHistoryCache,set:value=>{composerController.composerInputHistoryCache=value}},
"composerHistoryPicker":{configurable:true,get:()=>composerController.composerHistoryPicker,set:value=>{composerController.composerHistoryPicker=value}},
"composerUid":{configurable:true,get:()=>composerController.composerUid,set:value=>{composerController.composerUid=value}},
"composerDraftSeq":{configurable:true,get:()=>composerController.composerDraftSeq,set:value=>{composerController.composerDraftSeq=value}},
"lastMessageSelection":{configurable:true,get:()=>composerController.lastMessageSelection,set:value=>{composerController.lastMessageSelection=value}},
"lastMessageSelectionUid":{configurable:true,get:()=>composerController.lastMessageSelectionUid,set:value=>{composerController.lastMessageSelectionUid=value}},
"conversationSendEnabled":{configurable:true,get:()=>composerController.conversationSendEnabled,set:value=>{composerController.conversationSendEnabled=value}},
"newComposerDraft":{configurable:true,get:()=>composerController.newComposerDraft,set:value=>{composerController.newComposerDraft=value}},
"composerHydrations":{configurable:true,get:()=>composerController.composerHydrations,set:value=>{composerController.composerHydrations=value}},
"composerDraftWrites":{configurable:true,get:()=>composerController.composerDraftWrites,set:value=>{composerController.composerDraftWrites=value}},
"composerSaveQueues":{configurable:true,get:()=>composerController.composerSaveQueues,set:value=>{composerController.composerSaveQueues=value}},
"legacyComposerDatabase":{configurable:true,get:()=>composerController.legacyComposerDatabase,set:value=>{composerController.legacyComposerDatabase=value}},
"composerFollowBusy":{configurable:true,get:()=>composerController.composerFollowBusy,set:value=>{composerController.composerFollowBusy=value}},
"composerFollowedAt":{configurable:true,get:()=>composerController.composerFollowedAt,set:value=>{composerController.composerFollowedAt=value}},
"composerPendingSaves":{configurable:true,get:()=>composerController.composerPendingSaves,set:value=>{composerController.composerPendingSaves=value}},
"composerSaving":{configurable:true,get:()=>composerController.composerSaving,set:value=>{composerController.composerSaving=value}},
"composerRecoveryBusy":{configurable:true,get:()=>composerController.composerRecoveryBusy,set:value=>{composerController.composerRecoveryBusy=value}},
"composerUnloadWarning":{configurable:true,get:()=>composerController.composerUnloadWarning,set:value=>{composerController.composerUnloadWarning=value}},
"composerUnloadProtected":{configurable:true,get:()=>composerController.composerUnloadProtected,set:value=>{composerController.composerUnloadProtected=value}},
"composerServerRecoveries":{configurable:true,get:()=>composerController.composerServerRecoveries,set:value=>{composerController.composerServerRecoveries=value}},
"composerInputProbeBusy":{configurable:true,get:()=>composerController.composerInputProbeBusy,set:value=>{composerController.composerInputProbeBusy=value}},
"composerDraftSyncBusy":{configurable:true,get:()=>composerController.composerDraftSyncBusy,set:value=>{composerController.composerDraftSyncBusy=value}},
"COMPOSER_UPLOAD_LANES":{configurable:true,get:()=>composerController.COMPOSER_UPLOAD_LANES,set:value=>{composerController.COMPOSER_UPLOAD_LANES=value}},
"composerUploadLanes":{configurable:true,get:()=>composerController.composerUploadLanes,set:value=>{composerController.composerUploadLanes=value}},
"COMPOSER_PREVIEW_MAX_BYTES":{configurable:true,get:()=>composerController.COMPOSER_PREVIEW_MAX_BYTES,set:value=>{composerController.COMPOSER_PREVIEW_MAX_BYTES=value}},
"composerSending":{configurable:true,get:()=>composerController.composerSending,set:value=>{composerController.composerSending=value}},
"composerEscAt":{configurable:true,get:()=>composerController.composerEscAt,set:value=>{composerController.composerEscAt=value}},
"PASTE_CONFIRM_FILES":{configurable:true,get:()=>composerController.PASTE_CONFIRM_FILES,set:value=>{composerController.PASTE_CONFIRM_FILES=value}},
"PASTE_CONFIRM_BYTES":{configurable:true,get:()=>composerController.PASTE_CONFIRM_BYTES,set:value=>{composerController.PASTE_CONFIRM_BYTES=value}}
});

// 接管会话: 在服务端把它用 tmux resume 起来, 然后把终端嵌在会话详情底部。
// SSH 例外：PTY 在输入框上方。
// 会话跑在 tmux 里, 所以关掉页面/重启 sessiondock 都不会打断它。
// Hub 的节点侧连接/响应上限是 10 秒；再留出反向代理与浏览器调度余量。
// WebSocket 没有标准的建立超时，必须由页面回收永久 CONNECTING 的尝试。

// Claim includes browser/proxy transit plus the Hub's 5 s connect / 10 s read
// waits. A 5 s page deadline can cancel before the node even sees the request.

// DEC 2026 同步帧在页面这层暂存的上限：超过就先交给 xterm（它自己对 2026 还有
// 1 s 兜底），不让一个没收尾的帧无限占住输出。

// 每次页面加载独立生成；不写 local/sessionStorage，复制标签页也不会复制归属。
const TERM_PAGE_ID = window.__sessiondockPageId || crypto.randomUUID?.()
  || [...crypto.getRandomValues(new Uint8Array(16))]
    .map(value => value.toString(16).padStart(2, '0')).join('');

const terminalController = SessionDockTerminal.createController({
  pageId: TERM_PAGE_ID, gestures: SessionDockGestures,
  vendors: {
    get Terminal() { return window.Terminal; },
    get FitAddon() { return window.FitAddon; },
    get Unicode11Addon() { return window.Unicode11Addon; },
    get WebglAddon() { return window.WebglAddon; },
    ensureAssets: grid => window.ensureTerminalAssets?.(grid),
  },
  post: (...args) => post(...args),
  environment: {
    APP_BASE,
    HUB_MODE,
    MOBILE,
    Nodes,
    SessionDockCapabilities,
    SessionDockNetwork,
    store,
    SOURCES,
    appUrl: (...args) => appUrl(...args),
    applyNodeState: (...args) => applyNodeState(...args),
    nodeOf: (...args) => nodeOf(...args),
    newNodeId: (...args) => newNodeId(...args),
    sessionTerminalEnabled: (...args) => sessionTerminalEnabled(...args),
    sessiondockCli: (...args) => sessiondockCli(...args),
  },
  sessions: {
    S,
    ConsoleUI,
    cache,
    forkAncestors: (...args) => forkAncestors(...args),
    forkLeafUid: (...args) => forkLeafUid(...args),
    loadSessions: (...args) => loadSessions(...args),
    openSession: (...args) => openSession(...args),
    paintLive: (...args) => paintLive(...args),
    pendingTmuxSessions: (...args) => pendingTmuxSessions(...args),
    pendingUid: (...args) => pendingUid(...args),
    pollLive: (...args) => pollLive(...args),
    sidebarSessions: (...args) => sidebarSessions(...args),
    trimCache: (...args) => trimCache(...args),
    viewKey: (...args) => viewKey(...args),
    deleteSessions: (...args) => deleteSessions(...args),
  },
  composer: {
    get uid() { return composerUid; }, set uid(value) { composerUid = value; },
    get drafts() { return composerDrafts; },
    composerDraft: (...args) => composerDraft(...args),
    consolePasteFiles: (...args) => consolePasteFiles(...args),
    conversationSendEnabled: (...args) => conversationSendEnabled(...args),
    deleteComposerDraftStorage: (...args) => deleteComposerDraftStorage(...args),
    followServerDraft: (...args) => followServerDraft(...args),
    hydrateComposerDraft: (...args) => hydrateComposerDraft(...args),
    migrateComposerDraft: (...args) => migrateComposerDraft(...args),
    pollComposerInput: (...args) => pollComposerInput(...args),
    recoverComposerDrafts: (...args) => recoverComposerDrafts(...args),
    recoverServerComposerDrafts: (...args) => recoverServerComposerDrafts(...args),
    renderComposer: (...args) => renderComposer(...args),
    renderComposerItems: (...args) => renderComposerItems(...args),
    sendToSession: (...args) => sendToSession(...args),
    syncComposerDraftBindings: (...args) => syncComposerDraftBindings(...args),
    syncComposerUnloadProtection: (...args) => syncComposerUnloadProtection(...args),
  },
  presentation: {
    appAlert: (...args) => appAlert(...args),
    appConfirm: (...args) => appConfirm(...args),
    auditDetailRendered: (...args) => auditDetailRendered(...args),
    browserAuditEvent: (...args) => browserAuditEvent(...args),
    ensureConsolePlaceholder: (...args) => ensureConsolePlaceholder(...args),
    layoutHeader: (...args) => layoutHeader(...args),
    renderChips: (...args) => renderChips(...args),
    renderConversationTail: (...args) => renderConversationTail(...args),
    renderMachineSettings: (...args) => renderMachineSettings(...args),
    renderPendingSessionAction: (...args) => renderPendingSessionAction(...args),
    renderSide: (...args) => renderSide(...args),
    renderTakeoverBtn: (...args) => renderTakeoverBtn(...args),
    showConsoleToast: (...args) => showConsoleToast(...args),
    showMobileList: (...args) => showMobileList(...args),
    showNewSessionStage: (...args) => showNewSessionStage(...args),
    showSessionCount: (...args) => showSessionCount(...args),
    showSessionStopNotice: (...args) => showSessionStopNotice(...args),
  },
});
const T = terminalController.T;
// 旧版把 normal 当默认布局，无法区分“系统默认分屏”和“用户手动分屏”。
// 升级时只迁移一次；此后 normal 只会由拖动分界线产生并照常按会话保存。

/** Resolve one font face whose CJK glyph is exactly two Latin cells wide.
 * Ubuntu keeps its original glyph size: xterm reserves two cells for CJK and
 * rescaleOverlappingGlyphs prevents wide outlines from crossing cell bounds.
 * Other mixed stacks still prefer a locally available exact 1:2 font. */

// 保持色相、反射亮度；轻微曲线把中间色拉回 50%，避免彩色文字过艳。

// xterm 会回答 OSC 10/11/12/4 查询，答案走 term.onData，看起来像键盘输入。
// 把回包改写成暗色 palettes 再写回 PTY，会让 gh/survey 这类在查询后立刻
// 进 raw 读键的 CLI 把 ESC ] 当成非法按键（leftover 11;rgb:0000/0000/0000）。
// 丢掉回包：CLI 超时后沿用默认暗色 TUI，亮色页面只在浏览器反色，
// 也不把页面底色告诉 Codex/Claude/Grok。

function terminalColorChunk(...args) { return terminalController.terminalColorChunk(...args); }

function refreshTerminalPreferences(...args) { return terminalController.refreshTerminalPreferences(...args); }

// xterm 先测量网页字体再创建 DOM 行，避免按回退字体计算出错误的字符宽度。

function terminalListUncertain(...args) { return terminalController.terminalListUncertain(...args); }

function loadTermList(...args) { return terminalController.loadTermList(...args); }


function sessionTermMeta(...args) { return terminalController.sessionTermMeta(...args); }

/** Rust 下 pane 与 view 绑定的是接管/启动时核验的 uid，Codex 回退后不变；
 *  同一进程改写新分支后，分支叶子沿 forked_from_id 追溯到被绑定的祖先仍算同一
 *  控制台。 */


/** 返回会话所在的稳定 tmux pane 以及 pane 当前对应的 uid。
 *
 * 默认只接受 pane 的精确 uid 归属。Codex 回退后，同一个稳定 pane 会改绑到
 * 新叶子；只有负责跟进回退或用户明确切换终端的调用方才允许追随这个替代 uid。
 * Rust 后端的 pane 行始终写接管时绑定的 uid，叶子由列表的 fork 图推出。
 */
function linkedTermSession(...args) { return terminalController.linkedTermSession(...args); }

/** 某个会话是否已经被接管 (存在对应的 tmux 会话)。 */
function takenOver(...args) { return terminalController.takenOver(...args); }

/** Rust `outbox`: the reliable-send routes act under this page's
 *  own instance lease when the console is open here, so sending from the
 *  composer never conflicts with our own console. Without a lease the server
 *  claims for itself and reports any other page's lease as an ownership
 *  error. Undeclared capabilities send nothing extra. */
function termSendLease(...args) { return terminalController.termSendLease(...args); }

/** The pane's pinned identity for a page that holds no console lease here:
 *  the native `uid`+`instance_id` or the launch triple, exactly what attach
 *  would claim. The server then writes under its ordinary-claimant rule
 *  (refused while any page or server send holds the lease) instead of the
 *  unauthenticated `send-keys`. */


/** Rust `terminal_input`: raw HTTP text/keys under this page's exact terminal
 *  lease, or with an empty token plus the pane's pinned identity when the
 *  console is not open on this page (the composer's Esc and question cards,
 *  Grok text). Only the legacy HTTP shapes change (named keys, a bracketed
 *  `paste`, and raw text with `enter:false`); a Claude/Codex text submit is
 *  the reliable-send composer (see `termSendLease`), so a bare `text` returns
 *  null. Undeclared capabilities keep the body. */
function termInputBody(...args) { return terminalController.termInputBody(...args); }

/** tmux 列表已指向新分支时，原子跟进当前详情与输入状态。 */


/** 顶栏切换前先跟进 pane 的当前分支，然后再执行原本的对话/终端切换。 */
function toggleLinkedTermSession(...args) { return terminalController.toggleLinkedTermSession(...args); }

// ---------------------------------------------------------------- 接管
function takeover(...args) { return terminalController.takeover(...args); }

async function post(url, body, {timeoutMs = 0} = {}) {
  const traceId = String(body?.request_id || globalThis.crypto?.randomUUID?.()
    || `${Date.now()}-${Math.random().toString(36).slice(2)}`);
  const payload = { ...body, _build: BUILD_ID, _trace_id: traceId,
    _page_id: TERM_PAGE_ID };
  browserAuditEvent?.('http.request.started', {url, method: 'POST'}, null, {
    uid: body?.uid || '', traceId, requestId: body?.request_id || '',
  });
  const started = performance.now();
  let phase = 'headers', headersMs = null, status = null;
  const controller = timeoutMs ? new AbortController() : null;
  const timer = controller ? setTimeout(() => controller.abort(), timeoutMs) : null;
  try {
    const r = await fetch(appUrl(url), {
      method: 'POST', headers: {
        'Content-Type': 'application/json', 'X-SessionDock-Trace': traceId,
        'X-SessionDock-Page': TERM_PAGE_ID, 'X-SessionDock-Build': BUILD_ID,
      },
      body: JSON.stringify(payload),
      ...(controller ? {signal: controller.signal} : {}),
    });
    headersMs = Math.round((performance.now() - started) * 1000) / 1000;
    status = r.status;
    phase = 'body';
    browserAuditEvent?.('http.response.headers', {url, status, headers_ms: headersMs}, null,
      {uid: body?.uid || '', traceId, requestId: body?.request_id || ''});
    const text = await r.text();
    const bodyMs = Math.round((performance.now() - started - headersMs) * 1000) / 1000;
    phase = 'parse';
    let data;
    try {
      data = JSON.parse(text);
    } catch {
      throw new Error(r.status >= 500
        ? `SessionDock 请求失败（HTTP ${r.status}），请稍后重试`
        : 'SessionDock 返回了无法解析的响应，请重新加载');
    }
    browserAuditEvent?.('http.response.received', {
      url, status: r.status, ok: r.ok, headers_ms: headersMs,
      body_ms: bodyMs,
      duration_ms: Math.round((performance.now() - started) * 1000) / 1000,
    }, data, {uid: body?.uid || '', traceId, requestId: body?.request_id || '',
      severity: r.ok ? 'info' : 'warning'});
    if (data?.reload) markStaleBuild(data.build);
    return data;
  } catch (error) {
    if (controller?.signal.aborted) {
      error = new Error('请求超时，服务端可能已执行；请重新核对状态。');
      error.name = 'TimeoutError';
    }
    browserAuditEvent?.('http.request.failed', {
      url, error: String(error?.stack || error), phase, status, headers_ms: headersMs,
      timeout_ms: timeoutMs, online: navigator.onLine, visibility: document.visibilityState,
      duration_ms: Math.round((performance.now() - started) * 1000) / 1000,
    }, null, {uid: body?.uid || '', traceId, requestId: body?.request_id || '',
      severity: 'error'});
    throw error;
  } finally {
    if (timer !== null) clearTimeout(timer);
  }
}

// ---------------------------------------------------------------- 缺陷报告




function bugReportSource(...args) {return SessionUiLaunch.bugReportSource(...args);}

// 与新建会话一样，四种 AI CLI 都可以做处理会话；记住上次的选择，处理机器上
// 缺少的命令置灰（中央站下按所选机器的能力表，单机按本机）。


// 处理会话的机器下拉与新建会话一样列出全部机器。报告只有一份：还有没发出
// 的草稿时回到草稿所在的机器；否则默认选问题所在的机器（当前会话的机器），
// 其次上次的选择，再次唯一筛选中的机器。


// 记下正写着未发送内容的那份报告草稿在哪台机器上；清空或发出后忘掉。
function noteBugReportDraftNode(...args) {return SessionUiLaunch.noteBugReportDraftNode(...args);}



function showBugReportToast(...args) {return SessionUiLaunch.showBugReportToast(...args);}

// 报告框的附件复用对话输入框那一套：同样的选择菜单、粘贴/拖放、[附件N]
// 引用，以及同一个上传接口。处理会话的 cwd 固定为仓库根目录，因此上传
// 先流式暂存到服务端私有目录，点击发送后才发布到仓库 sessiondock_attachments/。
// newComposerDraft 定义在下方的对话输入框段落，只能在运行时按需创建。

function bindBugReportDraft(...args) {return SessionUiLaunch.bindBugReportDraft(...args);}



/** 下拉选的是跑处理会话的机器，不是另一份报告：正在写的描述、引用和附件
 *  跟着这次选择走，切换机器不清空输入框。服务端存储仍按机器分（附件的字节
 *  暂存在处理机器上），所以这里把内容搬到新机器的草稿上，再把原机器那份清
 *  空；目标机器上已有的服务端草稿由 hydrate 的早期编辑合并规则接上，两边
 *  都不丢。本页仍握着 File 的附件在新机器上重新暂存，原机器的暂存字节随
 *  清空后的草稿释放；重开页面后只剩服务端引用的卡片没有字节可重传，留在
 *  原机器的草稿里并在对话框上说明。返回要显示的提示文案。 */
function carryBugReportDraft(...args) {return SessionUiLaunch.carryBugReportDraft(...args);}

function renderBugReportItems(...args) {return SessionUiLaunch.renderBugReportItems(...args);}

function addBugReportFiles(...args) {return SessionUiLaunch.addBugReportFiles(...args);}

function clearBugReportDraft(...args) {return SessionUiLaunch.clearBugReportDraft(...args);}

// 处理会话（以及报告、附件）落在下拉里选中的机器上。
function bugReportNode(...args) {return SessionUiLaunch.bugReportNode(...args);}

// 问题所在的机器：当前会话的机器；没有会话时取唯一筛选中的机器；筛选着
// 多台机器又没选会话时说不清是哪台，按处理机器本身算。


// 报告里注明的问题机器；单机模式由服务端填主机名。




// 处理机器不是问题机器时，先向问题机器要一份服务端上下文（会话行、发送账本、
// 终端画面、审计窗口），随报告交给处理机器；问题机器离线或抓取失败时不阻断
// 报告，只把原因写进诊断包。




function openBugReportDialog(...args) {return SessionUiLaunch.openBugReportDialog(...args);}

// 详情标题栏是动态生成的，使用委托让列表页、普通会话和尚未落盘的
// 新会话共用同一个入口；手机进入详情后列表顶栏会被完整隐藏。





// 与 composer 一致：Enter 提交、Shift+Enter 换行；手机上 Enter 始终换行，只用按钮提交。












// 截图通常来自系统剪贴板；粘贴落在描述框或对话框内任意位置都接收。



function setSendButtonBusy(...args) {return SessionUiLaunch.setSendButtonBusy(...args);}





// ---------------------------------------------------------------- 新建会话


function commonSessionDirs(...args) {return SessionUiLaunch.commonSessionDirs(...args);}




function canCompleteCwd(...args){return SessionUiLaunch.canCompleteCwd(...args);}



function closeCwdPicker(...args) {return SessionUiLaunch.closeCwdPicker(...args);}









function renderCommonCwdOptions(...args) {return SessionUiLaunch.renderCommonCwdOptions(...args);}













// ---- 模型与推理强度选择器：每台机器、每个来源的 CLI 自己的列表 ----
// 新建会话和报告问题各用一个实例（元素 id 前缀不同）。两个控件始终在原位，
// 读取中/不可用时只是禁用并换文字，不改布局。





/** 模型按机器和来源记住；强度按来源和模型共享，不区分机器或弹窗。 */


const SessionUiLaunch=SessionDockSessionUi.createLaunch(
{$: (...args) => $(...args),
  get HUB_MODE() {return HUB_MODE;},
  get MOBILE() {return MOBILE;},
  get Nodes() {return Nodes;},
  get S() {return S;},
  get SOURCES() {return SOURCES;},
  get SessionDockCapabilities() {return SessionDockCapabilities;},
  get T() {return typeof T === 'undefined' ? undefined : T;},
  get TERM_PAGE_ID() {return TERM_PAGE_ID;},
  addDraftFiles: (...args) => addDraftFiles(...args),
  appConfirm: (...args) => appConfirm(...args),
  appUrl: (...args) => appUrl(...args),
  autoGrow: (...args) => autoGrow(...args),
  bindFileDrop: (...args) => bindFileDrop(...args),
  browserAuditEvent: (...args) => browserAuditEvent(...args),
  browserStateSnapshot: (...args) => browserStateSnapshot(...args),
  chooseAttachmentFiles: (...args) => chooseAttachmentFiles(...args),
  composerDraft: (...args) => composerDraft(...args),
  composerDraftOwner: (...args) => composerDraftOwner(...args),
  get composerDrafts() {return composerDrafts;},
  get composerHydrations() {return composerHydrations;},
  composerKindIcon: (...args) => composerKindIcon(...args),
  get composerPendingSaves() {return composerPendingSaves;},
  get composerSaveQueues() {return composerSaveQueues;},
  get composerSaving() {return composerSaving;},
  discardStagedAttachment: (...args) => discardStagedAttachment(...args),
  ensureComposerAttachmentNumbers: (...args) => ensureComposerAttachmentNumbers(...args),
  fmtSize: (...args) => fmtSize(...args),
  hydrateComposerDraft: (...args) => hydrateComposerDraft(...args),
  insertComposerReference: (...args) => insertComposerReference(...args),
  loadStagedComposerPreview: (...args) => loadStagedComposerPreview(...args),
  loadTermList: (...args) => loadTermList(...args),
  newDirsKey: (...args) => newDirsKey(...args),
  newNodeCapabilities: (...args) => newNodeCapabilities(...args),
  newNodeId: (...args) => newNodeId(...args),
  nodeOf: (...args) => nodeOf(...args),
  openPendingSession: (...args) => openPendingSession(...args),
  pasteAttachmentFiles: (...args) => pasteAttachmentFiles(...args),
  persistComposerDraft: (...args) => persistComposerDraft(...args),
  post: (...args) => post(...args),
  priorComposerSubmission: (...args) => priorComposerSubmission(...args),
  removeDraftAttachment: (...args) => removeDraftAttachment(...args),
  selectedNodeIds: (...args) => selectedNodeIds(...args),
  setComposerSendBusy: label => SessionDockComposer.setSendBusy(label),
  stageComposerAttachment: (...args) => stageComposerAttachment(...args),
  get staleBuildShown() {return typeof staleBuildShown === 'undefined' ? undefined : staleBuildShown;},
  get store() {return store;},
  takenOver: (...args) => takenOver(...args),
  termRows: (...args) => termRows(...args),
  uploadComposerAttachment: (...args) => uploadComposerAttachment(...args)});
const NewModels=SessionUiLaunch.NewModels;
const BugReportModels=SessionUiLaunch.BugReportModels;

function openNewSessionDialog(...args) {return SessionUiLaunch.openNewSessionDialog(...args);}

function selectPendingSidebarRow(uid, added) {
  const row = added ? null : document.querySelector(`#side .item[data-uid="${CSS.escape(uid)}"]`);
  if (!row) {
    renderSide();
    return;
  }
  document.querySelectorAll('#side .item.sel').forEach(item => item.classList.remove('sel'));
  row.classList.add('sel');
}

function showNewSessionStage(info) {
  if (!T.pendingModes.has(info.name)) T.pendingModes.set(info.name, T.mode);
  // 临时会话也必须完整切换视图；列表或字体慢时不能继续显示/操作旧终端。
  inflight?.abort();
  closeWatch();
  closeTermPane(true);
  progressDone();
  // create 返回后 term/list 可能还没拉完；先把服务端刚确认的新 tmux 放进本地
  // pending，详情页的终端切换、输入框和附件可以立即使用。
  const added = !T.pending.some(x => x.name === info.name);
  if (added) T.pending.push({ ...info, started: pendingStartedAt(info) });
  cancelSearch(true);
  S.sel = pendingUid(info.name);
  rememberComposerSession(composerDraft(S.sel), info);
  S.agent = null;
  store.set('sel', S.sel);
  store.set('agent', null);
  // Clicking an existing pending row must reach term/claim immediately. A full
  // sidebar rebuild can take seconds for large histories and blocks attach.
  selectPendingSidebarRow(S.sel, added);
  showSessionCount(sidebarSessions().length);
  const src = SOURCES[info.source];
  const pendingTitle = info.title || `新建 ${src.name} 会话`;
  $('#detail').replaceChildren(head({uid:S.sel,source:info.source,title:pendingTitle,node_name:info.node_name,cwd:info.cwd},0,info));
  const waiting=SessionDockSessionUi.pendingStage(SessionDockCapabilities.config.backend==='rust'?pendingStageMessage(info):'');
  $('#detail').appendChild(waiting);
  if (typeof auditDetailRendered === 'function') auditDetailRendered('new-session', {name: info.name});
  showMobileDetail();
  T.uid = S.sel;
  renderComposer();
  renderTakeoverBtn();
}

// Bug-report worker rows (`kind: "bug-report"`) carry the manifest status so
// an injection failure is visible instead of a silently idle CLI.

/** 启动型行（SSH 与代理的 receipt）唯一的状态来源。头部按钮、侧栏副标题、
 *  右键菜单和等待页文案都从这里取值，不各自拿 state/running 推断。
 *  本页刚观察到的宿主退出（T.ended，按实例）先于服务端落账生效，列表轮询
 *  回来一份仍写着 running 的旧行也不会把按钮翻回"停止"。 */
function pendingPhase(...args) { return terminalController.pendingPhase(...args); }

/** Sidebar meta text of a Rust pending row (other rows keep "等待首条消息"). */
function pendingStateLabel(...args) { return terminalController.pendingStateLabel(...args); }

// 等待页只说用户看得懂的事：启动、结束、停止、还没找到记录。关联方法、
// 证据和 uid 不出现在这里；后台每 1.5 s 自动重试关联，页面无需重试按钮。

function pendingStageMessage(...args) { return terminalController.pendingStageMessage(...args); }

// "还没找到记录"只在用户真的从本页发过消息一分钟后才说；首个回车是唯一依据。

function pendingSessionRow(...args) { return terminalController.pendingSessionRow(...args); }

/** SSH 会话和代理会话同一套结构：运行中是"停止"（先 Ctrl-D，再宿主停止；行保留、
 *  录制可回放），结束后是"删除"（discard，录制一并删）。其它待定行沿用"删除"（先停再丢弃）。 */
function pendingShellRunning(...args) { return terminalController.pendingShellRunning(...args); }

function pendingTitle(...args) { return terminalController.pendingTitle(...args); }

function renderPendingSessionAction(info, button = $('#a-session-action')) {
  if (!button || S.sel !== pendingUid(info.name)) return;
  // 行已不在列表里（别的页面删了、或已归档）：按已结束处理，绝不按旧 receipt 显示"停止"。
  const current = pendingSessionRow(info.name) || { ...info, running: false, stale: true, state: 'exited' };
  const stop = pendingShellRunning(current);
  const label = stop ? '停止会话' : '删除会话';
  SessionDockSessionUi.pendingAction({label,icon:stop?'power':'trash',run:(target,pending)=>stop?stopPendingSession(info,target,pending):deletePendingSession(info,target,pending)});

}

/** 本页观察到宿主退出后，页面立刻进入结束态（头部"删除"、副标题"已结束"），
 *  并强制刷一次列表让服务端落账跟上；不等下一轮轮询。 */


/** 选中的启动型行在两次列表之间从服务端消失（另一个页面删了它）：详情页不能
 *  留着旧头部。 */


function stopPendingSession(...args) { return terminalController.stopPendingSession(...args); }

function deletePendingSession(...args) { return terminalController.deletePendingSession(...args); }

function discardPendingSession(...args) { return terminalController.discardPendingSession(...args); }

/** SSH/shell receipts have no conversation archive; the PTY is the session. */
function sessionIsPtyOnly(...args) { return terminalController.sessionIsPtyOnly(...args); }

/** A CLI whose conversation SessionDock cannot read yet (OpenCode) shows its
 *  replies only in the PTY: the console leads the page like SSH, while the
 *  composer keeps the CLI's own send path and input checks. */
function sessionTerminalFirst(...args) { return terminalController.sessionTerminalFirst(...args); }

function openPendingSession(...args) { return terminalController.openPendingSession(...args); }




function createNewSession(...args) {return SessionUiLaunch.createNewSession(...args);}


















const termRows = terminalController.termRows;

/** 详情页头部那个按钮的文案随状态变。 */
function renderTakeoverBtn() {
  const b = $('#a-term');
  if (!b) return;
  const name = takenOver(S.sel);
  const replacement = name ? null
    : linkedTermSession(S.sel, { followReplacement: true });
  const paneOpen = !!name && !$('#termpane').classList.contains('hidden');
  const ptyOnly = typeof sessionTerminalFirst === 'function' && sessionTerminalFirst(S.sel);
  const switchToChat = paneOpen && (ptyOnly ? T.mode === 'full' : (MOBILE.matches || T.mode === 'full'));
  const terminalVisible = paneOpen && (ptyOnly || MOBILE.matches || T.mode !== 'collapsed');
  const label = replacement ? '切换到当前会话终端'
    : !name ? '接管会话'
    : ptyOnly ? (switchToChat ? '切换到对话' : '切换到终端')
    : MOBILE.matches ? (paneOpen ? '切换到对话' : '切换到终端')
    : switchToChat ? '切换到对话' : '切换到终端';
  b.innerHTML = uiIcon(switchToChat ? 'chat' : 'terminal');
  b.title = b.ariaLabel = label;
  b.setAttribute('aria-expanded', String(terminalVisible));
  b.classList.toggle('on', !!name);
  paintConsoleAvailability(b, S.sel, S.agent);
  composerController.renderComposer();
  if (typeof auditConsoleButton === 'function') auditConsoleButton('takeover-btn');
}

/** 终端面板每次开合/换模式都记一笔 terminal.pane：谁触发的、之前之后什么状态。 */


// ---------------------------------------------------------------- 终端面板
function currentTermViewObject(...args) { return terminalController.currentTermViewObject(...args); }

function syncTermAliases(...args) { return terminalController.syncTermAliases(...args); }

/** 只有显式打开终端的动作才能请求焦点；异步连接期间若用户已经点到别处，
 *  请求自动作废。这样 tmux 列表/会话正文的后台刷新不会打断搜索或编辑。 */

/** 用户选择的控制台渲染器：`grid` = 服务端网格（宿主解析，浏览器只画格子）。
 *  只有宿主声明支持网格（term/list 行的 `grid:true`）才用；旧宿主进程自动回退 xterm.js。 */
/** 这一行所在机器的控制台渲染：hub 按机器（中央注册表的 renderer），单机按本浏览器。默认服务端网格。 */

// 每个 WebSocket 包原样立即交给 xterm，页面这层不再攒 20 ms 合帧：xterm 自己
// 的 WriteBuffer 已按帧合并解析，Claude Code / Codex 的整屏重画都包在
// DEC 2026（synchronized output）里，由 xterm 压到一帧内绘制，不会再画出
// “先清行后重写”的中间态。攒批只会让每次按键回显固定多等一个定时器
// （实测 localhost p50 从 ~30 ms 降到 <1 ms，见 tests/bench_term_echo_browser.py）。
function writeTermOutput(...args) { return terminalController.writeTermOutput(...args); }

// 最后一个 ?2026h 之后没有 ?2026l 就算帧还开着。

/** Codex 的 side thread 目前可能只存在于正在运行的 TUI，绑定 main thread 的
 * 原生记录不会随它增长。只检查 xterm 已解析的实时屏幕底部，不能从原始包或
 * scrollback 搜索：重绘包会带旧内容，用户向上滚动也不代表 CLI 已切线程。 */


function codexSideThreadVisible(...args) { return terminalController.codexSideThreadVisible(...args); }

// Keep the PTY size stable under a soft keyboard, but do not bottom-align its
// blank tail. Short menus fit from the top; tall screens follow their content
// and cursor. Use parsed cells for both renderers, not CLI-specific text rules.
function positionTermViewport(...args) { return terminalController.positionTermViewport(...args); }

function termPaneRenderable(...args) { return terminalController.termPaneRenderable(...args); }

function refreshTerminalScale(...args) { return terminalController.refreshTerminalScale(...args); }

function fitTerm(...args) { return terminalController.fitTerm(...args); }

function restoreTermPane(...args) { return terminalController.restoreTermPane(...args); }

function openTermPane(...args) { return terminalController.openTermPane(...args); }

/** 顶栏按钮只切纯对话/纯终端；normal 分屏只能由用户拖动分界线产生。 */


// 同一个题目只自动呈现一次。用户看过以后仍可以主动切回原生终端；后台的
// 0.4 秒审批轮询和 tmux 列表刷新不能再把界面强行切回来。


/** 纯终端会遮住对话题卡。当前会话第一次收到待回答题目时切到对话；手动
 *  split 本来就能同时看到题卡，不改变它。prompt 结束后允许同一命令再次提问。 */
function revealConversationForPrompt(...args) { return terminalController.revealConversationForPrompt(...args); }

function closeTermPane(...args) { return terminalController.closeTermPane(...args); }

function layoutTermPane(...args) { return terminalController.layoutTermPane(...args); }

// `auto`：由布局恢复（选中会话、刷新、题卡）自动打开的 pty。进入对话页不该被
// 抢占问题打断：别处持有时静默放弃、留在对话页；只有用户主动打开 pty 才问。
function claimTermOwnership(...args) { return terminalController.claimTermOwnership(...args); }

// 抢占方的描述：浏览器拿不到主机名/用户名，服务端能给的只有 User-Agent 推出的
// 设备标签（"iPhone · Safari"）和地址；经 hub 访问时同一用户各页面地址相同，
// 所以服务端只在地址与本页不同时才给出地址。两样都没有就只说"另一页面"。
function describeTermTaker(...args) { return terminalController.describeTermTaker(...args); }

function handleTermRevoked(...args) { return terminalController.handleTermRevoked(...args); }

/** 在控制台面板里只读回放一段录制：不 claim、不发输入；网格视图收 JSON 行，xterm 视图收净化后的字节。 */


function sessionRecordingReplayable(...args) { return terminalController.sessionRecordingReplayable(...args); }

/** 录制回放的时间轴：进度条、播放/暂停、倍速、跳到最新。只对当前视图画。 */

function attachTerm(...args) { return terminalController.attachTerm(...args); }

// ---- 滚轮翻历史 ----

/** 退出滚动状态: 连带作废还没发出去和还在路上的滚动请求, 否则它们落地后
 *  会把 tmux 又推回 copy-mode。 */

/** 浏览器对 WebSocket 握手没有超时；永久 CONNECTING 必须退出本次租约并重走
 *  现有的存活核对/重连链路，不能据此把独立的 ptyhost 进程判成已退出。 */

/** OPEN only describes the local socket. Require a round trip through the node,
 * even when the CLI is quiet or output still arrives on a one-way connection.
 * Wait for the node's opt-in acknowledgement so old nodes never receive probes
 * as literal PTY input. Recovery replaces the transport, never replays keys. */


/** 网络短断后自动恢复。tmux 才是会话本体，WebSocket 只是可随时重建的视图。 */


/** 手机锁屏会冻结一个看似仍 OPEN、实际已经失效的 socket；恢复时必须强制换新。 */
function reconnectTerm(...args) { return terminalController.reconnectTerm(...args); }

function suspendTerm(...args) { return terminalController.suspendTerm(...args); }

function disposeTermView(...args) { return terminalController.disposeTermView(...args); }

// ---------------------------------------------------------------- 输入框
// 已接管的会话在消息流底部给个输入框, 不必展开整个终端就能说话。
function renderQueuedSends(uid = S.sel) {
  const box = $('#msgs'), stage = box ? null : $('#detail .new-session-wait');
  const draft = uid && uid === S.sel && !S.agent ? composerDrafts.get(composerDraftOwner(uid)) : null;
  const rows = Array.isArray(draft?.cli?.queued) ? draft.cli.queued : [];
  SessionDockConversation.stageQueue(stage,uid,rows);
  if (box) SessionDockConversation.tail(box,{queued:rows,returned:draft?.cli?.input?.code === 'cli_input_returned'});
}

function consolePasteFiles(view, name, e) {
  const files = clipboardAttachmentFiles(e.clipboardData);
  if (!files.length) return;
  if (!consolePasteFilesEnabled()) {
    showConsoleToast('已忽略粘贴的文件；在 设置 › 功能 开启「控制台粘贴文件」后会存入会话目录');
    setTimeout(() => {
      if ($('#console-toast')?.textContent.startsWith('已忽略粘贴的文件')) showConsoleToast('');
    }, 6000);
    return;
  }
  e.preventDefault();
  e.stopPropagation();
  if (T.name !== name || view.replay || view.ended || view.revoked) return;
  whenPasteConfirmed(files, () => {
    const uid = view.bindingUid || T.uid || '';
    if (!uid) { appAlert('粘贴文件失败：这个终端还没有会话目录'); return; }
    // One paste after another keeps its order, and each paste is one batch.
    const previous = consolePasteJobs.get(name) || Promise.resolve();
    const job = previous.then(() => publishConsolePaste(view, name, uid, files));
    consolePasteJobs.set(name, job.catch(() => {}));
  });
}

const consolePasteJobs = new Map();

function consoleAttachmentPath(document) {
  const relative = String(document.relative_path || '').replace(/^\.[\\/]/, '');
  const windows = document.path_style === 'windows';
  if (!relative) return String(document.path || '');
  if (windows) {
    const path = '.\\' + relative.replace(/\//g, '\\');
    return /\s/.test(path) ? `"${path}"` : path;
  }
  return ('./' + relative).replace(/\s/g, ch => '\\' + ch);
}

async function publishConsolePaste(view, name, uid, files) {
  const paths = [];
  let attachmentId = null;
  showConsoleToast(files.length === 1 ? `正在保存 ${files[0].name || '附件'}…` : `正在保存 ${files.length} 个文件…`);
  try {
    for (const file of files) {
      if (!file.size) throw new Error(`「${file.name || '附件'}」为空`);
      if (file.size > COMPOSER_MAX_FILE_BYTES) throw new Error(`「${file.name || '附件'}」超过 512 MB`);
      const url = new URL(appUrl('api/session/attachment'));
      url.searchParams.set('uid', uid);
      url.searchParams.set('name', file.name || 'attachment');
      if (attachmentId) url.searchParams.set('id', attachmentId);
      const response = await fetch(url, {
        method: 'POST', body: file,
        headers: {'Content-Type': file.type || 'application/octet-stream', 'X-SessionDock-Page': TERM_PAGE_ID},
      });
      let data = {};
      try { data = await response.json(); } catch { /* the status carries the failure */ }
      if (!response.ok || data.error) throw new Error(data.error || `HTTP ${response.status}`);
      attachmentId ||= data.attachment_id;
      paths.push(consoleAttachmentPath(data));
    }
  } catch (error) {
    showConsoleToast('');
    await appAlert('粘贴文件失败：' + (error.message || error));
    return;
  } finally {
    if ($('#console-toast')?.textContent.startsWith('正在保存')) showConsoleToast('');
  }
  if (T.name !== name || view.ended || view.revoked) return;
  const text = paths.join(' ') + ' ';
  browserAuditEvent?.('terminal.paste_files', {name, count: paths.length, attachment_id: attachmentId},
    null, {uid, connectionId: view.auditConnectionId || ''});
  try {
    const d = await post('api/term/send', termInputBody(name, {name, paste: text, uid}) || {name, paste: text});
    if (d.error) throw new Error(d.error);
  } catch (error) {
    await appAlert(`文件已保存到 ${paths.join(' ')}，但没有写进终端：` + (error.message || error));
  }
}

function bindFileDrop(zone, addFiles) {
  zone.addEventListener('dragenter', e => {
    if (e.dataTransfer?.types?.includes('Files')) zone.classList.add('dragover');
  });
  zone.addEventListener('dragover', e => {
    if (!e.dataTransfer?.types?.includes('Files')) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = 'copy';
  });
  zone.addEventListener('dragleave', e => {
    if (!zone.contains(e.relatedTarget)) zone.classList.remove('dragover');
  });
  zone.addEventListener('drop', e => {
    zone.classList.remove('dragover');
    const files = [...(e.dataTransfer?.files || [])];
    if (!files.length) return;
    e.preventDefault();
    addFiles(files);
  });
}
SessionDockTerminal.mount(document.querySelector("#terminal-root"));
composerController.start();

/** Match physical Alt: CSI modifier for navigation, ESC prefix for a character.
 * Ignore paste, mouse, focus and terminal replies; they are not the next key. */

/** 把 Ctrl 修饰的单个 ASCII 键转换为终端控制字节。多字符粘贴和中文不改写。 */

// ---- 高度拖动 ----

// 移动浏览器锁屏后常保留一个 readyState=OPEN 的僵尸 WebSocket。进入后台时主动
// 放弃这条传输，回到前台/pageshow/网络恢复时重新 attach；tmux 进程不会受影响。

function backgroundTerm(...args) { return terminalController.backgroundTerm(...args); }
function foregroundTerm(...args) { return terminalController.foregroundTerm(...args); }

// Native/global process discovery and managed terminal transport are independent
// Rust capabilities. The live poll normally refreshes this list; do not
// lose discovery of new/replacement hosts just because Rust keeps live:false.

terminalController.start();

$('#new-session').onclick=openNewSessionDialog;

document.addEventListener('click',event=>{if(event.target.closest('[data-report-bug]'))openBugReportDialog();});

Object.defineProperties(globalThis,{BUG_REPORT_DRAFT_UID:{configurable:true,get:()=>SessionUiLaunch.BUG_REPORT_DRAFT_UID},bugReportSending:{configurable:true,get:()=>SessionUiLaunch.bugReportSending},cwdCompletion:{configurable:true,get:()=>SessionUiLaunch.cwdCompletion}});

function bugReportDraftObject(...args) {return SessionUiLaunch.bugReportDraftObject(...args);}
