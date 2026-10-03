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
let bugReportToastTimer = 0;

const BUG_REPORT_SOURCES = { claude: 'Claude', codex: 'Codex', grok: 'Grok', opencode: 'OpenCode' };

function bugReportSource() {
  return $('#bug-report-source input:checked')?.value || 'codex';
}

// 与新建会话一样，四种 AI CLI 都可以做处理会话；记住上次的选择，处理机器上
// 缺少的命令置灰（中央站下按所选机器的能力表，单机按本机）。
function syncBugReportSources() {
  const remembered = store.get('bugReportSource', 'codex');
  const sources = HUB_MODE ? Nodes.capabilities[bugReportNode()]?.sources : T.sources;
  const known = sources && Object.keys(sources).length;
  let checked = null;
  for (const input of $('#bug-report-source').querySelectorAll('input')) {
    const missing = known && !sources[input.value];
    input.disabled = !!missing;
    input.title = missing ? `${bugReportNodeName() || '本机'}找不到 ${input.value} 命令` : '';
    if (input.value === remembered && !missing) checked = input;
  }
  const fallback = checked || [...$('#bug-report-source').querySelectorAll('input')]
    .find(input => !input.disabled);
  if (fallback) fallback.checked = true;
}

// 处理会话的机器下拉与新建会话一样列出全部机器。报告只有一份：还有没发出
// 的草稿时回到草稿所在的机器；否则默认选问题所在的机器（当前会话的机器），
// 其次上次的选择，再次唯一筛选中的机器。
function prepareBugReportNode() {
  if (!HUB_MODE) return;
  const select = $('#bug-report-node');
  const origin = bugReportOriginNode();
  const selected = selectedNodeIds();
  const drafted = store.get('bugReportDraftNode', '');
  const preferred = [drafted, origin, store.get('bugReportNode', ''),
    selected.length === 1 ? selected[0] : ''].filter(Boolean);
  select.replaceChildren();
  for (const n of Nodes.list) {
    const option = document.createElement('option');
    option.value = n.id;
    option.textContent = n.name + (Nodes.capabilities[n.id]?.enabled ? '' : '（离线）');
    option.disabled = !Nodes.capabilities[n.id]?.enabled;
    select.appendChild(option);
  }
  const usable = id => [...select.options].some(o => o.value === id && !o.disabled);
  select.value = preferred.find(usable) || [...select.options].find(o => !o.disabled)?.value || '';
  $('#bug-report-node-label').hidden = false;
  if (drafted && select.value !== drafted && Nodes.list.some(n => n.id === drafted)) {
    $('#bug-report-error').textContent =
      `${bugReportNodeName(drafted)} 离线，上面没发出的报告草稿要等它恢复后才能打开`;
  }
}

// 记下正写着未发送内容的那份报告草稿在哪台机器上；清空或发出后忘掉。
function noteBugReportDraftNode() {
  const node = nodeOf(BUG_REPORT_DRAFT_UID), draft = composerController.composerDrafts.get(BUG_REPORT_DRAFT_UID);
  if (!node || !draft || draft.loading) return;
  if (draft.text.trim() || draft.quotes.length || draft.attachments.length) {
    store.set('bugReportDraftNode', node);
  } else if (store.get('bugReportDraftNode', '') === node) {
    store.set('bugReportDraftNode', '');
  }
}

function bugReportNodeName(id = bugReportNode()) {
  return HUB_MODE ? Nodes.list.find(n => n.id === id)?.name || '' : '';
}

function showBugReportToast(report, worker) {
  const toast = $('#bug-report-toast');
  clearTimeout(bugReportToastTimer);
  toast.replaceChildren();
  const text = document.createElement('span');
  const label = BUG_REPORT_SOURCES[worker?.source] || '处理';
  const where = worker?.node_name ? `到 ${worker.node_name}` : '';
  text.textContent = `${report} 已保存${where}，${label} 处理会话正在启动`;
  const dismiss = () => {
    clearTimeout(bugReportToastTimer);
    toast.classList.add('hidden');
  };
  const open = document.createElement('button');
  open.type = 'button';
  open.className = 'btn primary';
  open.textContent = '打开';
  open.onclick = async () => {
    dismiss();
    await loadTermList();
    const pending = (T.pending || []).find(item => item.name === worker.name) || worker;
    await openPendingSession(pending);
  };
  const ignore = document.createElement('button');
  ignore.type = 'button';
  ignore.className = 'btn';
  ignore.textContent = '忽略';
  ignore.title = '关闭通知，处理会话继续运行';
  ignore.onclick = dismiss;
  const head = document.createElement('div');
  head.className = 'app-float-head';
  const title = document.createElement('strong');
  title.textContent = '缺陷报告已提交';
  head.appendChild(title);
  const actions = document.createElement('div');
  actions.className = 'app-float-actions';
  actions.append(ignore, open);
  toast.append(head, text, actions);
  toast.classList.remove('hidden');
  bugReportToastTimer = setTimeout(dismiss, 20000);
}

// 报告框的附件复用对话输入框那一套：同样的选择菜单、粘贴/拖放、[附件N]
// 引用，以及同一个上传接口。处理会话的 cwd 固定为仓库根目录，因此上传
// 先流式暂存到服务端私有目录，点击发送后才发布到仓库 sessiondock_attachments/。
// newComposerDraft 定义在下方的对话输入框段落，只能在运行时按需创建。
let BUG_REPORT_DRAFT_UID = '';
function bindBugReportDraft() {
  const node=bugReportNode();
  const key='reportDraftId.' + node;
  let id=store.get(key,'');
  if (!id) {id=crypto.randomUUID(); store.set(key,id);}
  BUG_REPORT_DRAFT_UID='report:' + (HUB_MODE ? node + '~' : '') + id;
}
let bugReportSending = false;
const bugReportDraftObject = () => composerController.composerDraft(BUG_REPORT_DRAFT_UID);

/** 下拉选的是跑处理会话的机器，不是另一份报告：正在写的描述、引用和附件
 *  跟着这次选择走，切换机器不清空输入框。服务端存储仍按机器分（附件的字节
 *  暂存在处理机器上），所以这里把内容搬到新机器的草稿上，再把原机器那份清
 *  空；目标机器上已有的服务端草稿由 hydrate 的早期编辑合并规则接上，两边
 *  都不丢。本页仍握着 File 的附件在新机器上重新暂存，原机器的暂存字节随
 *  清空后的草稿释放；重开页面后只剩服务端引用的卡片没有字节可重传，留在
 *  原机器的草稿里并在对话框上说明。返回要显示的提示文案。 */
function carryBugReportDraft(fromUid, toUid) {
  if (!fromUid || !toUid || fromUid === toUid || bugReportSending) return '';
  const from = composerController.composerDrafts.get(composerController.composerDraftOwner(fromUid));
  if (!from || from.handedOffSession) return '';
  const moving = from.attachments.filter(item => item.file instanceof Blob);
  const stranded = from.attachments.length - moving.length;
  if (!from.text && !from.quotes.length && !moving.length) return '';
  const to = composerController.composerDraft(toUid);
  if (!to) return '';
  const node = bugReportNode();
  to.text = to.text ? to.text + '\n' + from.text : from.text;
  to.quotes = [...to.quotes, ...from.quotes];
  to.attachments = [...to.attachments, ...moving];
  to.nextAttachmentNumber = Math.max(to.nextAttachmentNumber || 1, from.nextAttachmentNumber || 1);
  composerController.ensureComposerAttachmentNumbers(to);
  for (const field of ['requestId','requestText','report_prompt','report_text']) {
    delete to[field]; delete from[field];
  }
  from.attachments = from.attachments.filter(item => !moving.includes(item));
  from.text = ''; from.quotes = [];
  if (!from.attachments.length) from.nextAttachmentNumber = 1;
  const saved = composerController.persistComposerDraft(fromUid);
  for (const item of moving) {
    // 原机器上的暂存字节随清空后的草稿释放；新机器要的是一份新的上传。
    const previous = item.uploaded;
    if (item.cancelUpload) item.cancelUpload();
    item.uploaded = null; item.status = ''; item.error = '';
    composerController.discardStagedAttachment({uploaded: previous}, saved);
    const restage = () => {
      if (!to.attachments.includes(item)) return;
      // 被取消前已经落地的上传仍会写回 uploaded，再清一次才会重新暂存。
      if (item.uploaded) composerController.discardStagedAttachment(item, saved);
      item.uploaded = null; item.status = ''; item.error = '';
      composerController.stageComposerAttachment(item, toUid, {node, render: renderBugReportItems});
    };
    (item.staging || Promise.resolve()).then(restage, restage);
  }
  composerController.persistComposerDraft(toUid);
  if (!stranded) return '';
  const where = bugReportNodeName(nodeOf(fromUid)) || '原机器';
  return `${stranded} 个附件的文件只暂存在${where}，已留在那台机器的草稿里；`
    + '要随这份报告一起提交，请重新选择文件。';
}

function renderBugReportItems() {
  composerController.renderAttachmentCards($('#bug-report-items'), bugReportDraftObject().attachments, {
    uid: BUG_REPORT_DRAFT_UID, render: renderBugReportItems,
    disabled: bugReportSending,
    onInsert: number => composerController.insertComposerReference(number, $('#bug-report-description')),
    onRetry: attachment => composerController.stageComposerAttachment(attachment, BUG_REPORT_DRAFT_UID,
      {node: bugReportNode(), render: renderBugReportItems}),
    onRemove: id => {
      if (bugReportSending) return;
      const removed = composerController.removeDraftAttachment(bugReportDraftObject(), id);
      composerController.discardStagedAttachment(removed, composerController.persistComposerDraft(BUG_REPORT_DRAFT_UID));
      renderBugReportItems();
    },
  });
  composerController.renderSavedComposerInputs($('#bug-report-items'), bugReportDraftObject(), BUG_REPORT_DRAFT_UID);
  $('#bug-report-description').disabled = bugReportSending || !!bugReportDraftObject().loading;
  $('#bug-report-add').disabled = bugReportSending || !!bugReportDraftObject().loading;
  // 发送中换机器会把正在提交的内容搬走；锁住下拉直到这一次提交结束。
  $('#bug-report-node').disabled = bugReportSending;
}

function addBugReportFiles(files) {
  const draft = bugReportDraftObject();
  const before = new Set(draft.attachments);
  composerController.addDraftFiles(draft, files);
  composerController.persistComposerDraft(BUG_REPORT_DRAFT_UID);
  for (const attachment of draft.attachments) {
    if (!before.has(attachment)) {
      composerController.stageComposerAttachment(attachment, BUG_REPORT_DRAFT_UID, {node: bugReportNode(), render: renderBugReportItems});
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
  const saved=composerController.persistComposerDraft(BUG_REPORT_DRAFT_UID);
  for (const attachment of dropped) composerController.discardStagedAttachment(attachment, saved);
  renderBugReportItems();
}

// 处理会话（以及报告、附件）落在下拉里选中的机器上。
function bugReportNode() {
  return HUB_MODE ? $('#bug-report-node').value || '' : '';
}

// 问题所在的机器：当前会话的机器；没有会话时取唯一筛选中的机器；筛选着
// 多台机器又没选会话时说不清是哪台，按处理机器本身算。
function bugReportOriginNode() {
  if (!HUB_MODE) return '';
  const fromSession = nodeOf(S.sel);
  if (fromSession) return fromSession;
  const candidates = selectedNodeIds();
  return candidates.length === 1 ? candidates[0] : '';
}

// 报告里注明的问题机器；单机模式由服务端填主机名。
function bugReportOrigin(workerNode) {
  const uid = S.sel || '';
  if (!HUB_MODE) return {uid};
  const nodeId = bugReportOriginNode() || workerNode;
  return {node_id: nodeId, node_name: bugReportNodeName(nodeId), uid};
}

function bugReportNodeError(node) {
  if (!HUB_MODE) return '';
  if (!node) return '没有可用的机器：请先在顶部选择一台在线机器或打开一个会话';
  const info = Nodes.list.find(n => n.id === node);
  if (info?.online === false) {
    return `${info.name || '所选机器'} 离线，无法在该机器上保存报告；请换一台在线机器`;
  }
  if (!Nodes.capabilities[node]?.enabled) {
    return `${info?.name || '所选机器'} 未启用终端，无法启动处理会话；请换一台机器`;
  }
  return '';
}

// 处理机器不是问题机器时，先向问题机器要一份服务端上下文（会话行、发送账本、
// 终端画面、审计窗口），随报告交给处理机器；问题机器离线或抓取失败时不阻断
// 报告，只把原因写进诊断包。
async function captureBugReportContext(originNode, uid, terminalName) {
  try {
    const d = await post('api/bug-report/capture', {
      _node: originNode, uid, terminal_name: terminalName, page_id: TERM_PAGE_ID,
    });
    if (d.error) return {error: d.error};
    return d;
  } catch (failure) {
    return {error: `抓取失败：${failure.message || failure}`};
  }
}

function closeBugReportAttachMenu() {
  $('#bug-report-attach-menu').classList.add('hidden');
  $('#bug-report-form .report-composer').style.removeProperty('padding-top');
  $('#bug-report-add').classList.remove('on');
  $('#bug-report-add').setAttribute('aria-expanded', 'false');
}

function openBugReportDialog() {
  const dialog = $('#bug-report-dialog');
  $('#bug-report-error').textContent = '';
  $('#bug-report-go').disabled = typeof staleBuildShown !== 'undefined' && staleBuildShown;
  $('#bug-report-go').textContent = '发送';
  setSendButtonBusy($('#bug-report-go'), '');
  prepareBugReportNode();
  bindBugReportDraft();
  renderBugReportItems();
  $('#bug-report-description').value = bugReportDraftObject().text;
  composerController.hydrateComposerDraft(BUG_REPORT_DRAFT_UID).then(async () => {
    const handedOff=bugReportDraftObject().handedOffSession;
    if (handedOff) {
      showBugReportToast(handedOff.report_id,handedOff);
      store.set('reportDraftId.'+bugReportNode(),crypto.randomUUID());bindBugReportDraft();
      await composerController.hydrateComposerDraft(BUG_REPORT_DRAFT_UID);
    }
    if (!bugReportSending) {
      $('#bug-report-description').value = bugReportDraftObject().text;
      renderBugReportItems();
      composerController.autoGrow($('#bug-report-description'));
    }
  });
  syncBugReportSources();
  modelCatalogs.clear(); // 每次打开都读一遍：CLI 升级或换配置后列表会变
  BugReportModels.refresh();
  dialog.showModal();
  closeBugReportAttachMenu();
  composerController.autoGrow($('#bug-report-description'));
  setTimeout(() => $('#bug-report-description').focus(), 0);
}

// 详情标题栏是动态生成的，使用委托让列表页、普通会话和尚未落盘的
// 新会话共用同一个入口；手机进入详情后列表顶栏会被完整隐藏。
document.addEventListener('click', event => {
  if (!event.target.closest('[data-report-bug]')) return;
  openBugReportDialog();
});
$('#bug-report-node').onchange = () => {
  const previous = BUG_REPORT_DRAFT_UID;
  store.set('bugReportNode', bugReportNode());
  bindBugReportDraft();
  const notice = carryBugReportDraft(previous, BUG_REPORT_DRAFT_UID);
  $('#bug-report-description').value=bugReportDraftObject().text;
  renderBugReportItems();
  composerController.hydrateComposerDraft(BUG_REPORT_DRAFT_UID);
  syncBugReportSources();
  BugReportModels.refresh();
  $('#bug-report-error').textContent = notice;
  composerController.autoGrow($('#bug-report-description'));
};
$('#bug-report-source').addEventListener('change', () => {
  store.set('bugReportSource', bugReportSource());
  BugReportModels.refresh();
});
$('#bug-report-dialog .modal-close').onclick = () => $('#bug-report-dialog').close();
$('#bug-report-description').addEventListener('input', event => {
  bugReportDraftObject().text = event.target.value;
  composerController.persistComposerDraft(BUG_REPORT_DRAFT_UID);
  composerController.autoGrow(event.target);
});
// 与 composer 一致：Enter 提交、Shift+Enter 换行；手机上 Enter 始终换行，只用按钮提交。
$('#bug-report-description').addEventListener('keydown', e => {
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing && !MOBILE.matches) {
    e.preventDefault();
    $('#bug-report-form').requestSubmit();
  }
});
window.addEventListener('resize', () => {
  if ($('#bug-report-dialog').open) composerController.autoGrow($('#bug-report-description'));
});
let bugReportBackdropPressed = false;
function bugReportBackdropHit(event) {
  const dialog = $('#bug-report-dialog');
  const rect = dialog.getBoundingClientRect();
  return event.target === dialog && (event.clientX < rect.left || event.clientX >= rect.right
    || event.clientY < rect.top || event.clientY >= rect.bottom);
}
$('#bug-report-dialog').addEventListener('pointerdown', event => {
  bugReportBackdropPressed = event.button === 0 && bugReportBackdropHit(event);
});
$('#bug-report-dialog').addEventListener('pointercancel', () => { bugReportBackdropPressed = false; });
$('#bug-report-dialog').addEventListener('close', () => { bugReportBackdropPressed = false; });
$('#bug-report-dialog').addEventListener('click', event => {
  // A selection dragged from the form to the backdrop also targets the dialog.
  // Dismiss only when the gesture both starts and ends on the actual backdrop.
  const dismiss = bugReportBackdropPressed && bugReportBackdropHit(event);
  bugReportBackdropPressed = false;
  if (dismiss) $('#bug-report-dialog').close();
});
$('#bug-report-add').onclick = event => {
  event.stopPropagation();
  const menu = $('#bug-report-attach-menu');
  const open = menu.classList.toggle('hidden');
  $('#bug-report-add').classList.toggle('on', !open);
  $('#bug-report-add').setAttribute('aria-expanded', String(!open));
  const row = $('#bug-report-form .report-composer');
  if (open) row.style.removeProperty('padding-top');
  else {
    row.style.paddingTop = `${menu.offsetHeight + 7}px`;
    $('#bug-report-add').scrollIntoView({block: 'nearest'});
  }
};
$('#bug-report-attach-menu').onclick = event => {
  const button = event.target.closest('button[data-attach]');
  if (!button) return;
  closeBugReportAttachMenu();
  composerController.chooseAttachmentFiles(button.dataset.attach, $('#bug-report-file'), addBugReportFiles);
};
$('#bug-report-file').onchange = event => {
  addBugReportFiles([...event.target.files]);
  event.target.value = '';
};
$('#bug-report-dialog').addEventListener('click', event => {
  if (!event.target.closest('#bug-report-dialog .attach-picker')) closeBugReportAttachMenu();
});
// 截图通常来自系统剪贴板；粘贴落在描述框或对话框内任意位置都接收。
$('#bug-report-form').addEventListener('paste', event => composerController.pasteAttachmentFiles(event, addBugReportFiles));
bindFileDrop($('#bug-report-form'), addBugReportFiles);

function setSendButtonBusy(button, label) {
  if (!button) return;
  button.setAttribute('aria-busy', label ? 'true' : 'false');
  if (label) button.setAttribute('aria-label', label);
  else button.removeAttribute('aria-label');
}

function completeBugReportSubmission(data,node) {
  const oldUid=BUG_REPORT_DRAFT_UID,draft=composerController.composerDrafts.get(oldUid);
  if (draft) {
    for (const item of draft.attachments) if (item.preview) URL.revokeObjectURL(item.preview);
    composerController.composerSaveQueues.delete(draft);composerController.composerPendingSaves.delete(draft);composerController.composerSaving.delete(draft);
  }
  composerController.composerDrafts.delete(oldUid);composerController.composerHydrations.delete(oldUid);
  if (store.get('bugReportDraftNode', '') === node) store.set('bugReportDraftNode', '');
  store.set('reportDraftId.'+node,crypto.randomUUID());bindBugReportDraft();
  $('#bug-report-description').value='';
  showBugReportToast(data.report_id,data.worker);$('#bug-report-dialog').close();
  void loadTermList();
}

$('#bug-report-form').onsubmit = async event => {
  event.preventDefault();
  if (bugReportSending) return;
  const reportText = $('#bug-report-description').value;
  const description = reportText.trim();
  const error = $('#bug-report-error');
  if (!description) {
    error.textContent = '请先描述遇到的问题';
    $('#bug-report-description').focus();
    return;
  }
  const button = $('#bug-report-go');
  const attachments = [...bugReportDraftObject().attachments];
  const node = bugReportNode();
  const nodeError = bugReportNodeError(node);
  if (nodeError) {
    error.textContent = nodeError;
    return;
  }
  const origin = bugReportOrigin(node);
  const remote = HUB_MODE && !!origin.node_id && origin.node_id !== node;
  if (typeof staleBuildShown !== 'undefined' && staleBuildShown) {
    error.textContent = '页面已更新，请重新加载后再提交';
    return;
  }
  bugReportSending = true;
  button.disabled = true;
  setSendButtonBusy(button, '发送中');
  $('#bug-report-add').disabled = true;
  error.textContent = '';
  renderBugReportItems();
  const snapshot = browserStateSnapshot('bug-report');
  browserAuditEvent('bug_report.requested', {
    ...snapshot.data, attachments: attachments.length,
    worker_node: node, origin_node: origin.node_id || '', remote,
  }, snapshot.content);
  try {
    bugReportDraftObject().text = reportText;
    await composerController.hydrateComposerDraft(BUG_REPORT_DRAFT_UID);
    const pending=bugReportDraftObject();
    const priorPayload=JSON.stringify({description,source:bugReportSource(),...BugReportModels.choice(),
      attachments:attachments.map(a=>({upload_id:a.uploaded?.upload_id,number:a.number})),origin});
    if (pending.requestId && pending.requestText===priorPayload) {
      const previous=await composerController.priorComposerSubmission(BUG_REPORT_DRAFT_UID,pending.requestId,true);
      if (previous?.worker) {completeBugReportSubmission(previous,node);return;}
    }
    if (!await composerController.persistComposerDraft(BUG_REPORT_DRAFT_UID)) throw new Error(bugReportDraftObject().storageError || '报告草稿保存失败');
    // 与对话发送一致：同一批附件共用一个编号目录，失败的附件保留在卡片上重试。
    const uploaded = [];
    let attachmentId = null;
    for (let i = 0; i < attachments.length; i++) {
      setSendButtonBusy(button, `上传 ${i + 1}/${attachments.length}`);
      if (attachments[i].staging) await attachments[i].staging;
      const result = await composerController.uploadComposerAttachment(
        attachments[i], BUG_REPORT_DRAFT_UID, attachmentId, { node, render: renderBugReportItems });
      attachmentId ||= result.attachment_id;
      uploaded.push({upload_id:result.upload_id,number:attachments[i].number});
    }
    const terminalName = takenOver(S.sel) || (T.uid === S.sel ? T.name : '') || '';
    let captured = null;
    if (remote) {
      setSendButtonBusy(button, '抓取中');
      captured = await captureBugReportContext(origin.node_id, S.sel || '', terminalName);
    }
    setSendButtonBusy(button, '提交中');
    const source = bugReportSource();
    store.set('bugReportSource', source);
    const reportDraft=bugReportDraftObject();
    const choice = BugReportModels.choice();
    const requestText=JSON.stringify({description,source,...choice,attachments:uploaded,origin});
    if (reportDraft.requestText!==requestText || !reportDraft.requestId) {
      reportDraft.requestText=requestText;reportDraft.requestId=crypto.randomUUID();
    }
    if (!await composerController.persistComposerDraft(BUG_REPORT_DRAFT_UID)) throw new Error(reportDraft.storageError || '报告草稿保存失败');
    // 远端抓取时会话与终端引用不再随请求下发：中央站要求 uid 与 _node 指向
    // 同一台机器，问题会话的引用改由 origin.uid 与 captured 携带。
    const d = await post('api/bug-report', {
      ...(HUB_MODE ? {_node: node} : {}),
      draft_uid:BUG_REPORT_DRAFT_UID,draft_revision:reportDraft.revision,request_id:reportDraft.requestId,
      description, uid: remote ? '' : (S.sel || ''), page_id: TERM_PAGE_ID, source, ...choice,
      terminal_name: remote ? '' : terminalName, snapshot, attachments: uploaded,
      origin, ...(captured ? {captured} : {}),
      cols: Math.max(80, T.term?.cols || 120), rows: Math.max(24, T.term?.rows || 36),
    });
    if (d.error) {
      error.textContent = d.error;
      return;
    }
    completeBugReportSubmission(d,node);
  } catch (failure) {
    error.textContent = `提交失败：${failure.message || failure}`;
  } finally {
    bugReportSending = false;
    button.disabled = false;
    setSendButtonBusy(button, '');
    $('#bug-report-add').disabled = false;
    renderBugReportItems();
  }
};

// ---------------------------------------------------------------- 新建会话
function suggestedSessionDir(cwd) {
  const path = String(cwd || '').replace(/\/+$/, '') || '/';
  // CLI/SDK 经常在这些易失根目录里生成一次性测试会话。它们仍属于会话
  // 历史，但不该因一次自动任务污染“最近使用”的新建目录建议。
  return path.startsWith('/') && !['/tmp', '/var/tmp', '/dev/shm'].some(
    root => path === root || path.startsWith(root + '/'));
}

function commonSessionDirs() {
  const dirs = new Map();
  for (const s of S.sessions) {
    if (HUB_MODE && s.node_id !== newNodeId()) continue;
    const cwd = String(s.cwd || '');
    if (!suggestedSessionDir(cwd)) continue;
    const row = dirs.get(cwd) || { cwd, count: 0, updated: '' };
    row.count++;
    if ((s.updated || '') > row.updated) row.updated = s.updated || '';
    dirs.set(cwd, row);
  }
  for (const [i, cwd] of store.get(newDirsKey(), []).entries()) {
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
  const input = $('#new-cwd'), picker = $('#new-cwd-picker');
  cancelCwdCompletionRequest();
  cwdCompletion.rows = [];
  cwdCompletion.completions = [];
  cwdCompletion.forValue = '';
  cwdCompletion.active = -1;
  $('#new-cwd-options').replaceChildren();
  picker.hidden = true;
  input.setAttribute('aria-expanded', 'false');
  input.removeAttribute('aria-activedescendant');
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
  const input = $('#new-cwd'), picker = $('#new-cwd-picker');
  const box = $('#new-cwd-options');
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
  box.replaceChildren();
  $('#new-cwd-options-title').textContent = value ? '匹配目录' : '最近使用';
  picker.hidden = false;

  const section = label => {
    const heading = document.createElement('div');
    heading.className = 'new-cwd-section';
    heading.setAttribute('role', 'presentation');
    heading.textContent = label;
    box.appendChild(heading);
  };
  const note = message => {
    const messageNode = document.createElement('div');
    messageNode.className = 'new-cwd-empty';
    messageNode.textContent = message;
    box.appendChild(messageNode);
  };
  const addOption = ({path, meta, kind}, index) => {
    const option = document.createElement('button');
    option.type = 'button';
    option.id = `new-cwd-option-${index}`;
    option.className = 'new-cwd-option';
    option.dataset.cwdKind = kind;
    option.dataset.cwdOption = String(index);
    option.setAttribute('role', 'option');
    option.setAttribute('aria-selected', 'false');
    option.title = path;
    const label = document.createElement('span');
    label.className = 'new-cwd-option-path';
    label.textContent = path;
    option.appendChild(label);
    if (meta) {
      const detail = document.createElement('span');
      detail.className = 'new-cwd-option-meta';
      detail.textContent = meta;
      option.appendChild(detail);
    }
    box.appendChild(option);
  };

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
  input.setAttribute('aria-expanded', 'true');
  input.removeAttribute('aria-activedescendant');
  $('#new-cwd-completion-status').textContent = options.length
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
  const options = [...$('#new-cwd-options').querySelectorAll('[data-cwd-option]')];
  options.forEach((option, index) => {
    const active = index === next;
    option.classList.toggle('active', active);
    option.setAttribute('aria-selected', String(active));
  });
  const option = options[next];
  $('#new-cwd').setAttribute('aria-activedescendant', option.id);
  option.scrollIntoView({ block: 'nearest' });
}

function setCwdValue(value, refresh = true) {
  const input = $('#new-cwd');
  input.value = value;
  $('#new-session-error').textContent = '';
  input.focus();
  input.setSelectionRange(value.length, value.length);
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
  const input = $('#new-cwd');
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
  const value = input.value.trim();
  const prefix = longestCommonPrefix(rows);
  if (prefix.length > value.length) {
    setCwdValue(prefix, false);
    cwdCompletion.forValue = prefix;
    $('#new-cwd-completion-status').textContent =
      `已补全公共前缀，仍有 ${rows.length} 个补全建议`;
  }
}

async function loadCwdCompletions(complete = false) {
  cancelCwdCompletionRequest();
  const input = $('#new-cwd');
  const value = input.value.trim();
  const recent = matchingRecentCwdOptions(value);
  if (SessionDockCapabilities.config.backend === 'rust' && !SessionDockCapabilities.allows('terminal_complete_dir')) {
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
    const response = await fetch(appUrl(`api/term/complete-dir?${params}`),
      { signal: controller.signal, cache: 'no-store' });
    const data = await response.json();
    if (sequence !== cwdCompletion.sequence || input.value.trim() !== value) return;
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
  const value = $('#new-cwd').value.trim();
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

// ---- 模型与推理强度选择器：每台机器、每个来源的 CLI 自己的列表 ----
// 新建会话和报告问题各用一个实例（元素 id 前缀不同）。两个控件始终在原位，
// 读取中/不可用时只是禁用并换文字，不改布局。
const MODEL_SEARCH_MIN = 10;
const modelCatalogs = new Map();

function fetchModelCatalog(node, source) {
  const key = (HUB_MODE ? node + '|' : '') + source;
  if (!modelCatalogs.has(key)) {
    modelCatalogs.set(key, (async () => {
      try {
        const params = new URLSearchParams({source, ...(HUB_MODE ? {node} : {})});
        const response = await fetch(appUrl(`api/term/models?${params}`), {cache: 'no-store'});
        const data = response.ok ? await response.json() : null;
        return Array.isArray(data?.models) ? data : null;
      } catch { return null; }
    })().then(catalog => {
      if (!catalog) modelCatalogs.delete(key); // 下次打开再试
      return catalog;
    }));
  }
  return modelCatalogs.get(key);
}

/** 模型按机器和来源记住；强度按来源和模型共享，不区分机器或弹窗。 */
function createModelPicker(prefix, {source, node, storeKey}) {
  const el = name => document.getElementById(`${prefix}-${name}`);
  const button = el('model'), label = el('model-label'), menu = el('model-menu');
  const search = el('model-search'), box = el('model-options'), select = el('effort');
  const picker = {catalog: null, key: '', model: '', effort: '', rows: [], active: -1, seq: 0};
  const info = id => picker.catalog?.models.find(model => model.id === id) || null;
  const controls = (text, title, enabled) => {
    label.textContent = text;
    label.classList.toggle('default', !enabled);
    button.title = title;
    button.disabled = !enabled;
  };
  picker.apply = (model, effort, persist = false) => {
    const catalog = picker.catalog, chosen = info(model);
    const shown = chosen || info(catalog?.default_model);
    picker.model = shown?.id || '';
    const efforts = shown?.efforts || catalog?.efforts || [];
    const effortKey = `modelEffort.${source()}|${picker.model}`;
    const remembered = store.get(effortKey, '');
    picker.effort = efforts.includes(effort) ? effort : efforts.includes(remembered) ? remembered
      : efforts.includes('high') ? 'high'
      : efforts.includes(shown?.default_effort) ? shown.default_effort : efforts[0] || '';
    const name = shown ? shown.name || shown.id : catalog?.models.length ? '选择模型' : '模型不可用';
    controls(name, shown && shown.name !== shown.id ? `${shown.name}（${shown.id}）` : '模型：' + name,
      !!catalog?.models.length);
    select.replaceChildren(...efforts.map(value => new Option(value, value)));
    select.value = picker.effort;
    select.disabled = !efforts.length;
    select.parentElement.title = efforts.length ? '推理强度' : '该 CLI 不支持选择推理强度';
    if (persist) store.set(`${storeKey}.${picker.key}`, {model: picker.model});
    if (persist && effort && picker.model && efforts.includes(effort)) store.set(effortKey, effort);
  };
  picker.refresh = async () => {
    const current = source(), where = node(), seq = ++picker.seq;
    picker.close();
    picker.key = (HUB_MODE ? where + '|' : '') + current;
    picker.catalog = null;
    picker.model = picker.effort = '';
    picker.apply('', '');
    if (!current || current === 'shell') {
      controls('模型不可用', '终端会话不选择模型', false);
      return;
    }
    controls('读取模型…', '正在读取该 CLI 的模型列表', false);
    const catalog = await fetchModelCatalog(where, current);
    if (seq !== picker.seq) return;
    picker.catalog = catalog;
    const saved = store.get(`${storeKey}.${picker.key}`, {}) || {};
    picker.apply(saved.model || '', '');
    if (!catalog) controls('模型不可用', '该机器没有返回模型列表，将使用 CLI 默认模型', false);
  };
  /** 请求体里的具体选择；目录不可用时才由 CLI 自行决定。 */
  picker.choice = () => ({...(picker.model ? {model: picker.model} : {}),
    ...(picker.effort ? {effort: picker.effort} : {})});
  picker.close = (focus = false) => {
    if (menu.hidden) return;
    if (menu.matches(':popover-open')) menu.hidePopover();
    menu.hidden = true;
    button.setAttribute('aria-expanded', 'false');
    if (focus) button.focus();
  };
  const setActive = (index, scroll = true) => {
    const options = [...box.querySelectorAll('[data-model-option]')];
    picker.active = options.length ? (index + options.length) % options.length : -1;
    options.forEach((option, i) => option.classList.toggle('active', i === picker.active));
    const active = options[picker.active];
    for (const target of [search, box]) {
      if (active) target.setAttribute('aria-activedescendant', active.id);
      else target.removeAttribute('aria-activedescendant');
    }
    if (active && scroll) active.scrollIntoView({block: 'nearest'});
  };
  const render = () => {
    const query = search.value.trim().toLocaleLowerCase();
    const all = picker.catalog.models;
    picker.rows = query ? all.filter(model => model.id
      && `${model.id} ${model.name || ''}`.toLocaleLowerCase().includes(query)) : all;
    box.replaceChildren();
    picker.rows.forEach((model, index) => {
      const option = document.createElement('button');
      option.type = 'button';
      option.id = `${prefix}-model-option-${index}`;
      option.className = 'new-model-option';
      option.dataset.modelOption = String(index);
      option.setAttribute('role', 'option');
      option.setAttribute('aria-selected', String(model.id === picker.model));
      option.tabIndex = -1;
      option.title = model.id;
      const name = document.createElement('span');
      name.textContent = model.name || model.id;
      option.appendChild(name);
      if (model.id && model.name && model.name !== model.id) {
        const id = document.createElement('small');
        id.textContent = model.id;
        option.appendChild(id);
      }
      box.appendChild(option);
    });
    if (!picker.rows.length) {
      const empty = document.createElement('div');
      empty.className = 'new-model-empty';
      empty.textContent = '没有匹配的模型';
      box.appendChild(empty);
    }
    const selected = picker.rows.findIndex(model => model.id === picker.model);
    setActive(query ? 0 : Math.max(0, selected), !query);
  };
  const open = () => {
    if (!picker.catalog?.models.length) return;
    search.hidden = picker.catalog.models.length <= MODEL_SEARCH_MIN;
    search.value = '';
    menu.hidden = false;
    menu.showPopover();
    button.setAttribute('aria-expanded', 'true');
    render();
    place();
    (search.hidden ? box : search).focus();
  };
  // 浮层与模型+强度这一组等宽，下方放不下时翻到上方。getBoundingClientRect 是缩放后的
  // 像素，写回 style 前除以界面缩放。
  const place = () => {
    const zoom = menu.currentCSSZoom || 1, gap = 4, edge = 8;
    const group = button.closest('.new-choice').getBoundingClientRect();
    const pick = button.getBoundingClientRect();
    const below = innerHeight - pick.bottom - gap - edge, above = pick.top - gap - edge;
    const up = below < 160 && above > below;
    menu.style.left = `${group.left / zoom}px`;
    menu.style.width = `${group.width / zoom}px`;
    menu.style.maxHeight = `${Math.max(0, up ? above : below) / zoom}px`;
    const top = up ? pick.top - gap - menu.getBoundingClientRect().height : pick.bottom + gap;
    menu.style.top = `${top / zoom}px`;
  };
  addEventListener('resize', () => { if (!menu.hidden) place(); });
  addEventListener('scroll', e => { if (!menu.hidden && !menu.contains(e.target)) place(); }, true);
  const choose = index => {
    const model = picker.rows[index];
    if (!model) return;
    picker.apply(model.id, '', true);
    picker.close(true);
  };
  button.onclick = () => (menu.hidden ? open() : picker.close());
  button.onkeydown = e => {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); open(); }
  };
  search.oninput = render;
  menu.onkeydown = e => {
    if (e.isComposing) return;
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      setActive(picker.active + (e.key === 'ArrowDown' ? 1 : -1));
    } else if (e.key === 'Enter') {
      e.preventDefault();
      choose(picker.active);
    } else if (e.key === 'Escape') {
      // 只收起下拉，不关闭整个对话框。
      e.preventDefault();
      e.stopPropagation();
      picker.close(true);
    } else if (e.key === 'Tab') {
      picker.close();
    }
  };
  box.onclick = e => {
    const option = e.target.closest('[data-model-option]');
    if (option) choose(Number(option.dataset.modelOption));
  };
  select.onchange = e => picker.apply(picker.model, e.currentTarget.value, true);
  const dialog = button.closest('dialog');
  dialog.addEventListener('pointerdown', e => {
    if (!button.parentElement.contains(e.target)) picker.close();
  });
  dialog.addEventListener('close', () => picker.close());
  return picker;
}

const NewModels = createModelPicker('new', {
  source: () => $('#new-session-dialog input[name="new-source"]:checked')?.value || '',
  node: () => newNodeId(), storeKey: 'newModel'});
const BugReportModels = createModelPicker('bug-report', {
  source: () => bugReportSource(), node: () => bugReportNode(), storeKey: 'bugReportModel'});

function openNewSessionDialog() {
  const dialog = $('#new-session-dialog');
  closeCwdPicker();
  newCreateAttempt = null;
  modelCatalogs.clear(); // 每次打开都读一遍：CLI 升级或换配置后列表会变
  prepareNewNode();
  refreshNewNodeFields();
  NewModels.refresh();
  dialog.showModal();
  renderCommonCwdOptions();
  setTimeout(() => { $('#new-cwd').focus(); $('#new-cwd').select(); }, 0);
}

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
  if (added) T.pending.push({ ...info, started: composerController.pendingStartedAt(info) });
  cancelSearch(true);
  S.sel = pendingUid(info.name);
  composerController.rememberComposerSession(composerController.composerDraft(S.sel), info);
  S.agent = null;
  store.set('sel', S.sel);
  store.set('agent', null);
  // Clicking an existing pending row must reach term/claim immediately. A full
  // sidebar rebuild can take seconds for large histories and blocks attach.
  selectPendingSidebarRow(S.sel, added);
  showSessionCount(sidebarSessions().length);
  const src = SOURCES[info.source];
  const pendingTitle = info.title || `新建 ${src.name} 会话`;
  $('#detail').innerHTML = `<div class="dhead"><div class="dtitle">
    <button class="mobile-back" title="返回会话列表" aria-label="返回会话列表">←</button>
    <h2>${sessionIconMarkup(info.source, true, true)}<span>${esc(pendingTitle)}</span></h2>
    <div class="dhead-actions" aria-label="会话操作">
      <button class="iconbtn" id="a-term" title="切换到终端" aria-label="切换到终端">${uiIcon('terminal')}</button>
      ${sessionActionsMarkup(`
      <button class="session-menu-action" data-report-bug title="报告当前会话问题"
        aria-label="报告当前会话问题">${uiIcon('bug')}</button>
      <button class="session-menu-action danger" id="a-session-action"></button>
      `, `
    <div class="dmeta"><span id="mcount-total">0 条消息</span>
      ${info.node_name ? `<span class="meta-node node-badge" data-node-color="${nodeColor(info.node_name)}">${esc(info.node_name)}</span>` : ''}
      <span class="meta-secondary"><code>${esc(shortCwd(info.cwd, 999))}</code></span>
      <span class="meta-source">${esc(src.name)}</span></div>`)}
    </div></div>
  </div><div class="empty new-session-wait">${SessionDockCapabilities.config.backend === 'rust'
    ? esc(pendingStageMessage(info)) : ''}</div>`;
  $('#detail .mobile-back').onclick = showMobileList;
  bindConsoleButton($('#a-term'), S.sel);
  renderPendingSessionAction(info);
  bindSessionActions($('#detail .dhead'));
  if (typeof auditDetailRendered === 'function') auditDetailRendered('new-session', {name: info.name});
  showMobileDetail();
  T.uid = S.sel;
  composerController.renderComposer();
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
  button.innerHTML = uiIcon(stop ? 'power' : 'trash');
  button.title = button.ariaLabel = label;
  if (typeof labelSessionAction === 'function') labelSessionAction(button);
  button.onclick = () => stop ? stopPendingSession(current, button) : deletePendingSession(current, button);
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

let newCreateAttempt = null;
function newSessionRequestId(source, cwd) {
  const key = JSON.stringify([newNodeId(), source, cwd, NewModels.model, NewModels.effort]);
  if (newCreateAttempt?.key !== key) newCreateAttempt = {key,
    rows: termRows(),
    id: globalThis.crypto?.randomUUID?.() || `${Date.now()}-${Math.random().toString(36).slice(2)}`};
  return newCreateAttempt.id;
}

async function createNewSession(e) {
  e.preventDefault();
  const source = $('#new-session-dialog input[name="new-source"]:checked')?.value;
  const cwd = $('#new-cwd').value.trim();
  const go = $('#new-session-go'), error = $('#new-session-error');
  error.textContent = '';
  if (!source) { error.textContent = '没有可用的会话类型'; return; }
  if (!cwd) { error.textContent = '请选择启动目录'; return; }
  go.disabled = true;
  go.textContent = '创建中…';
  const dirsKey = newDirsKey();
  try {
    const requestId = newSessionRequestId(source, cwd);
    const request = { source, cwd, cols: 120, rows: newCreateAttempt.rows,
      request_id: requestId, ...(HUB_MODE ? {_node: newNodeId()} : {}),
      ...NewModels.choice() };
    let d = await post('api/term/create', request);
    if (d.needs_create) {
      const target = String(d.cwd || cwd);
      if (!await appConfirm(`启动目录不存在：\n${target}\n\n是否创建该目录并继续？`)) {
        $('#new-cwd').focus();
        return;
      }
      go.textContent = '创建目录中…';
      d = await post('api/term/create', { ...request, cwd: target, create_cwd: true });
    }
    if (d.error) { error.textContent = d.error; return; }
    const recent = [d.cwd, ...store.get(dirsKey, []).filter(x => x !== d.cwd)].slice(0, 8);
    store.set(dirsKey, recent);
    $('#new-session-dialog').close();
    await openPendingSession(d);
    // The backend instance is ready; list refresh must not block the conversation.
    void loadTermList();
  } catch (err) {
    error.textContent = err.message || '创建失败';
  } finally {
    go.disabled = false;
    go.textContent = '创建';
  }
}

$('#new-session').onclick = openNewSessionDialog;
$('#new-session-form').onsubmit = createNewSession;
$('#new-session-dialog .modal-close').onclick = () => $('#new-session-dialog').close();
$('#new-session-dialog .modal-cancel').onclick = () => $('#new-session-dialog').close();
$('#new-cwd').oninput = () => {
  $('#new-session-error').textContent = '';
  scheduleCwdCompletions();
};
$('#new-cwd').onkeydown = e => {
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
};
$('#new-cwd-options').onclick = e => {
  const option = e.target.closest('[data-cwd-option]');
  if (!option) return;
  e.preventDefault();
  const path = cwdCompletion.rows[Number(option.dataset.cwdOption)];
  if (path) setCwdValue(path);
};
$('#new-session-dialog').addEventListener('close', closeCwdPicker);
$('#new-session-form .new-source').addEventListener('change', () => NewModels.refresh());
$('#new-node').addEventListener('change', () => NewModels.refresh());
let newSessionBackdropPressed = false;
function newSessionBackdropHit(event) {
  const dialog = $('#new-session-dialog');
  const rect = dialog.getBoundingClientRect();
  return event.target === dialog && (event.clientX < rect.left || event.clientX >= rect.right
    || event.clientY < rect.top || event.clientY >= rect.bottom);
}
$('#new-session-dialog').addEventListener('pointerdown', event => {
  newSessionBackdropPressed = event.button === 0 && newSessionBackdropHit(event);
});
$('#new-session-dialog').addEventListener('pointercancel', () => { newSessionBackdropPressed = false; });
$('#new-session-dialog').addEventListener('close', () => { newSessionBackdropPressed = false; });
$('#new-session-dialog').addEventListener('click', event => {
  // Like the report dialog, require both ends of the gesture on the backdrop.
  // Selecting input text and releasing outside also targets the dialog.
  const dismiss = newSessionBackdropPressed && newSessionBackdropHit(event);
  newSessionBackdropPressed = false;
  if (dismiss) $('#new-session-dialog').close();
});

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
function renderQueuedSends(uid = composerUid) {
  // A new session waits on its stage page until the first native record;
  // its queued text goes under the stage text instead of a message list.
  const box = $('#msgs'), stage = box ? null : $('#detail .new-session-wait');
  if (box) {
    const draft = uid && uid === S.sel && !S.agent ? composerDrafts.get(composerDraftOwner(uid)) : null;
    SessionDockConversation.tail(box,{queued:Array.isArray(draft?.cli?.queued) ? draft.cli.queued : [],returned:draft?.cli?.input?.code === 'cli_input_returned'});
    return;
  }
  $('#queued-sends')?.remove();
  if (!box && !stage) return;
  const draft = uid && uid === S.sel && !S.agent ? composerDrafts.get(composerDraftOwner(uid)) : null;
  const rows = Array.isArray(draft?.cli?.queued) ? draft.cli.queued : [];
  if (!rows.length) return;
  const block = el('div', 'queued-sends');
  block.id = 'queued-sends';
  block.setAttribute('role', 'status');
  block.setAttribute('aria-live', 'polite');
  for (const item of rows) {
    const lost = item.state === 'lost';
    const interrupted = item.state === 'interrupted';
    const n = el('div', 'msg queued-send' + (lost ? ' lost' : ''));
    n.dataset.role = 'user';
    n.dataset.requestId = item.request_id;
    n.dataset.state = item.state || 'queued';
    const body = el('div', 'mb');
    const text = String(item.text || '');
    if (typeof md === 'function') body.innerHTML = md(text, true, [], {uid, agent:null});
    else body.textContent = text;
    n.appendChild(body);
    // The CLI's own enqueue record means it holds the text until its current step ends.
    const inCli = !lost && !interrupted && item.cli_queued_at != null;
    if (inCli) n.dataset.cliQueued = '1';
    const state = el('small', 'queued-send-state', lost ? '未送达，请到终端查看'
      : interrupted ? 'CLI 已中断，未确认处理，请到终端查看'
      : inCli ? '已进入 CLI 队列，当前步骤结束后处理' : '已发送，等待 CLI 处理');
    if (lost || interrupted) {
      const close = el('button', 'queued-send-dismiss', '关闭');
      close.type = 'button';
      close.onclick = () => dismissQueuedSend(uid, item.request_id);
      state.appendChild(close);
    }
    n.appendChild(state);
    block.appendChild(n);
  }
  stage.insertAdjacentElement('afterend', block);
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
