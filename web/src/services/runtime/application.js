import {GridTerm} from '../../../../legacy-web/grid/facade.js'
import {startControlEvents} from './controls'
import * as SessionUi from '../../migration/session-ui'
import * as Overlays from '../../migration/overlays'
import * as Composer from '../../migration/composer'
import * as Terminal from '../../migration/terminal'
import * as Shell from '../../migration/shell'
import * as Search from '../../migration/search'
import * as Sidebar from '../../migration/sidebar'
import * as Conversation from '../../migration/conversation'
import * as Settings from '../../migration/settings'
import * as Machines from '../../migration/machines'
import {runtimePinia} from '../../stores/runtime/pinia'
import {useRuntimePresentationStore} from '../../stores/runtime/presentation'
import {createRuntimeCore} from './core'
import {createEnvironment} from './environment.js'
import {createPreferences} from './preferences'
import {createPostService} from './post.js'
import {createMetadataService} from './metadata.js'
import {createBulkOperations} from './bulk.js'
import {createBuildService} from './build'
import {createAppearance} from './appearance'
import {createShellEnvironment} from './shell-environment'
import {mountRuntimeNotices} from './mount-notices'
import {startNetworkLifecycle} from './network-lifecycle'
import {createSessionFilters} from '../../domain/runtime/session-filters.js'
import {createDetailNotices,showDetailState} from './detail-notices'
import {createSidebarResources} from './sidebar-resources.js'
import {createDom} from './dom.js'
import {createTimeline} from './timeline.js'
import {createStatus} from './status.js'
import {createCatalogMeta} from './catalog-meta.js'
import {createSidebarGestures} from './sidebar-gestures.js'
import {createSidebarView} from './sidebar-view.js'
import {createConversationRenderer} from './conversation-renderer.js'
import {createSyntaxRuntime} from './syntax-runtime.js'
import {createFormulaRuntime} from './formula-runtime.js'
import {createQuestionRuntime} from './question-runtime.js'
import {createMarkdownRuntime} from './markdown-runtime.js'
import {createMediaRuntime} from './media-runtime.js'
import {createSettingsRuntime} from './settings-runtime.js'
import {createTakeover} from './takeover.js'
import {createPendingStage} from './pending-stage.js'
import {createConsolePaste} from './console-paste.js'
import * as Cli from '../../domain/runtime/cli.js'
import * as Format from '../../domain/runtime/format.js'
import {SOURCES} from '../../domain/runtime/sources'
import {sessiondockCli} from '../../domain/runtime/cli.js'
import {viewKey} from '../../domain/runtime/messages.js'
import {pendingUid} from '../../domain/runtime/pending'
import {shortCwd} from '../../domain/runtime/format.js'
import {retryableReadFailure} from '../../domain/runtime/read-failures.js'

// Bootstrap only composes explicit scoped owners. Their native caches, protocol
// instances, pending deliveries and timers remain inside the named services.
export function createRuntimeApplication(){
 const capabilities=Overlays.capabilities, presentation=useRuntimePresentationStore(runtimePinia)
 let core,terminal,composer,sessionUi,launch,metadata,bulk,post,build,appearance,sleep,resources,filters,
   dom,timeline,status,catalogMeta,sidebarGestures,sidebarView,conversationRenderer,syntaxRuntime,formulaRuntime,
   questionRuntime,markdownRuntime,mediaRuntime,settingsRuntime,takeover,pendingStage,consolePaste,sidebarResources,detailNotices
 const environment=createEnvironment(capabilities,()=>core.nodes.selectedNodeIds(),()=>core.nodes.newNodeId())
 const preferences=createPreferences(capabilities,environment.STORAGE_PREFIX)
 questionRuntime=createQuestionRuntime()
 composer=Composer.createController({environment,preferences,capabilities,runtime:()=>core,terminal:()=>terminal,
   composer:()=>composer,launch:()=>launch,build:()=>build,post:()=>post,dom:()=>dom,questionRuntime:()=>questionRuntime,pendingStage:()=>pendingStage,
   sidebarView:()=>sidebarView,status:()=>status,takeover:()=>takeover,sleep:()=>sleep})
 const terminalOperations={get state(){return terminal?.state},get vendors(){return vendors},ensureAssets:Overlays.ensureTerminalAssets,
   terminalListUncertain:(...args)=>terminal.terminalListUncertain(...args),loadTermList:(...args)=>terminal.loadTermList(...args),
   closeTermPane:(...args)=>terminal.closeTermPane(...args),revealConversationForPrompt:(...args)=>terminal.revealConversationForPrompt(...args),
   takeover:(...args)=>terminal.takeover(...args),sessionRecordingReplayable:(...args)=>terminal.sessionRecordingReplayable(...args),
   sessionTermMeta:(...args)=>terminal.sessionTermMeta(...args),linkedTermSession:(...args)=>terminal.linkedTermSession(...args),
   pendingPhase:(...args)=>terminal.pendingPhase(...args),toggleLinkedTermSession:(...args)=>terminal.toggleLinkedTermSession(...args),
   renderTakeoverBtn:()=>takeover.renderTakeoverBtn()}
 const launchOperations={canCompleteCwd:(...args)=>launch.canCompleteCwd(...args),closeCwdPicker:(...args)=>launch.closeCwdPicker(...args),
   get cwdCompletion(){return launch.cwdCompletion},commonSessionDirs:(...args)=>launch.commonSessionDirs(...args),renderCommonCwdOptions:(...args)=>launch.renderCommonCwdOptions(...args)}
 const sidebarOperations={ensureSidebarVue:()=>sidebarView.ensureSidebarVue(),updateNodes:Sidebar.updateNodes,
   renderView:()=>sidebarView.renderView(),renderSide:()=>sidebarView.renderSide(),renderPickBar:()=>bulk.renderPickBar(),updateGroupMenu:Sidebar.updateGroupMenu}
 const views={consoleButton:presentation.consoleButton,shortCwd,esc:(...args)=>dom.esc(...args),uiIcon:(...args)=>dom.uiIcon(...args),appAlert:SessionUi.appAlert,
   status:Search.status,ensureSidebarVue:()=>sidebarView.ensureSidebarVue(),loadFailed:Sidebar.loadFailed,
   refreshSessionMeta:()=>catalogMeta.refreshSessionMeta(),renderChips:()=>sidebarView.renderChips(),renderSide:(...args)=>sidebarView.renderSide(...args),
   showSessionCount:(...args)=>status.showSessionCount(...args),patchSide:(...args)=>sidebarView.patchSide(...args),visible:()=>filters.visible(),paintLive:()=>status.paintLive(),
   sealTurnTail:(...args)=>conversationRenderer.sealTurnTail(...args),renderConversationTail:(...args)=>conversationRenderer.renderConversationTail(...args),
   renderSessionAction:(...args)=>sessionUi.renderSessionAction(...args),head:(...args)=>sessionUi.head(...args),layoutSessionHead:()=>sessionUi.layoutSessionHead(),
   paintTurn:(...args)=>status.paintTurn(...args),renderSession:(...args)=>conversationRenderer.renderSession(...args),
   appendMessages:(...args)=>Conversation.append(...args),markMatches:(...args)=>Search.markMatches(...args),updateMatchNav:(...args)=>Search.updateMatchNav(...args),
   sidebarTextSelectionProtected:()=>sidebarGestures.sidebarTextSelectionProtected(),refreshSidebarRows:(...args)=>sidebarView.refreshSidebarRows(...args),
   showMobileDetail:Shell.showMobileDetail,showMobileList:Shell.showMobileList,ensureConsolePlaceholder:()=>conversationRenderer.ensureConsolePlaceholder(),
   syncSessionFreezeOverlay:()=>sessionUi.syncSessionFreezeOverlay(),syncSessionStopNotice:()=>sessionUi.syncSessionStopNotice(),
   revealSessionInSidebar:(...args)=>sidebarView.revealSessionInSidebar(...args),paintSidebarSelection:(...args)=>sidebarView.paintSidebarSelection(...args),
   progress:(...args)=>conversationRenderer.progress(...args),progressDone:()=>conversationRenderer.progressDone(),renderTakeoverBtn:()=>takeover.renderTakeoverBtn()}
 core=createRuntimeCore(runtimePinia,{initialEnvironment:environment,initialPreferences:preferences,capabilities,terminal:terminalOperations,
   composer:{drafts:composer.composerDrafts,applyCliState:(...args)=>composer.applyCliState(...args),hide:()=>composer.hide(),migrateComposerDraft:(...args)=>composer.migrateComposerDraft(...args)},
   launch:launchOperations,sidebar:sidebarOperations,itemMenu:{get uid(){return sidebarGestures?.menuUid||''},close:()=>sidebarGestures.closeItemMenu(),setGroupExpanded:expanded=>sidebarGestures.setGroupExpanded(expanded)},presentation:views,
   conversation:{messageIndex:Conversation.index.messageIndex,markInterruptedTurn:Conversation.planning.markInterruptedTurn,
     flushPendingTurnSeal:(...args)=>conversationRenderer.flushPendingTurnSeal(...args),validateHistoryPage:Conversation.pages.validateHistoryPage},
   rendering:{cancel:()=>conversationRenderer.cancel(),clearSyntax:()=>syntaxRuntime.syntaxQueue.clear(),clearFormulae:()=>formulaRuntime.formulaRoots.clear()},
   el:(...args)=>dom.el(...args)})
 const vendors={get Terminal(){return window.Terminal},get FitAddon(){return window.FitAddon},get Unicode11Addon(){return window.Unicode11Addon},get WebglAddon(){return window.WebglAddon},ensureAssets:Overlays.ensureTerminalAssets}
 terminal=Terminal.createController({fetch:(...args)=>core.network.fetch(...args),pageId:environment.AUDIT_PAGE_ID,gestures:Shell.gestures,vendors,
   post:(...args)=>post.post(...args),environment:{...environment,Nodes:core.state.nodes,SessionDockCapabilities:capabilities,SessionDockNetwork:core.network,store:preferences,SOURCES,
     applyNodeState:(...args)=>core.nodes.applyNodeState(...args),nodeOf:(...args)=>core.nodes.nodeOf(...args),newNodeId:()=>core.nodes.newNodeId(),sessionTerminalEnabled:(...args)=>core.nodes.sessionTerminalEnabled(...args),sessiondockCli},
   sessions:{state:core.state,ConsoleUI:core.state.console,cache:core.cache.cache,
     forkAncestors:(...args)=>core.index.forkAncestors(...args),forkLeafUid:(...args)=>core.index.forkLeafUid(...args),loadSessions:(...args)=>core.list.loadSessions(...args),openSession:(...args)=>core.open.openSession(...args),paintLive:()=>status.paintLive(),pendingTmuxSessions:()=>core.pending.pendingTmuxSessions(),pendingUid,
     pollLive:(...args)=>core.live.pollLive(...args),sidebarSessions:()=>core.index.sidebarSessions(),trimCache:()=>core.cache.trimCache(),viewKey,deleteSessions:(...args)=>bulk.deleteSessions(...args)},
   composer:{get uid(){return composer.composerUid},set uid(value){composer.composerUid=value},drafts:composer.composerDrafts,
     composerDraft:(...args)=>composer.composerDraft(...args),consolePasteFiles:(...args)=>consolePaste.consolePasteFiles(...args),conversationSendEnabled:()=>composer.conversationSendEnabled(),
     deleteComposerDraftStorage:(...args)=>composer.deleteComposerDraftStorage(...args),followServerDraft:(...args)=>composer.followServerDraft(...args),hydrateComposerDraft:(...args)=>composer.hydrateComposerDraft(...args),migrateComposerDraft:(...args)=>composer.migrateComposerDraft(...args),pollComposerInput:()=>composer.pollComposerInput(),recoverComposerDrafts:()=>composer.recoverComposerDrafts(),recoverServerComposerDrafts:()=>composer.recoverServerComposerDrafts(),renderComposer:()=>composer.renderComposer(),renderComposerItems:()=>composer.renderComposerItems(),sendToSession:(...args)=>composer.sendToSession(...args),syncComposerDraftBindings:()=>composer.syncComposerDraftBindings(),syncComposerUnloadProtection:()=>composer.syncComposerUnloadProtection()},
   presentation:{appAlert:SessionUi.appAlert,appConfirm:SessionUi.appConfirm,auditDetailRendered:(...args)=>core.diagnostics.auditDetailRendered(...args),browserAuditEvent:(...args)=>core.audit.browserAuditEvent(...args),
     ensureConsolePlaceholder:views.ensureConsolePlaceholder,layoutHeader:Shell.layoutHeader,renderChips:views.renderChips,renderConversationTail:views.renderConversationTail,
     renderMachineSettings:Machines.renderMachineSettings,renderPendingSessionAction:(...args)=>pendingStage.renderPendingSessionAction(...args),renderSide:views.renderSide,renderTakeoverBtn:views.renderTakeoverBtn,
     showConsoleToast:SessionUi.consoleToast,showMobileList:Shell.showMobileList,setPendingStage:SessionUi.setPendingStage,showDetailState,showNewSessionStage:(...args)=>pendingStage.showNewSessionStage(...args),showSessionCount:views.showSessionCount,showSessionStopNotice:(...args)=>sessionUi.showSessionStopNotice(...args)}})
 build=createBuildService(runtimePinia,{buildId:environment.BUILD_ID,appBase:environment.APP_BASE,network:core.network,fetch:(...args)=>core.network.fetch(...args),appUrl:environment.appUrl,audit:(...args)=>core.audit.browserAuditEvent(...args),prepareComposerReload:()=>composer.prepareComposerReload(),disableSending:()=>{Composer.disableSending();SessionUi.disableBugReportSending()}})
 post=createPostService({fetch:(...args)=>core.network.fetch(...args),appUrl:environment.appUrl,pageId:environment.AUDIT_PAGE_ID,buildId:environment.BUILD_ID,browserAuditEvent:(...args)=>core.audit.browserAuditEvent(...args),markStaleBuild:build.markStaleBuild})
 dom=createDom({core,status:()=>status,capabilities})
 const sharedUi={environment,preferences,capabilities,runtime:()=>core,terminal:()=>terminal,composer:()=>composer,sessionUi:()=>sessionUi,launch:()=>launch,
   bulk:()=>bulk,metadata:()=>metadata,dom:()=>dom,sidebarView:()=>sidebarView,sidebarGestures:()=>sidebarGestures,status:()=>status,conversationRenderer:()=>conversationRenderer,pendingStage:()=>pendingStage,takeover:()=>takeover,post:()=>post,build:()=>build,
   bindFileDrop:(...args)=>consolePaste.bindFileDrop(...args)}
 sessionUi=SessionUi.createAppControllers(sharedUi,sharedUi,sharedUi)
 metadata=createMetadataService(runtimePinia,{...environment,fetch:(...args)=>core.network.fetch(...args),capabilities},core.state,core.cache,core.index,core.nodes,core.sync,
   {renderChips:views.renderChips,renderSide:views.renderSide,showSessionCount:views.showSessionCount,renderSessionAction:views.renderSessionAction,renderForkChainMenu:(...args)=>sessionUi.renderForkChainMenu(...args),appAlert:SessionUi.appAlert,paintStarButton:(...args)=>sessionUi.paintStarButton(...args),status:Search.status})
 bulk=createBulkOperations(runtimePinia,{...environment,fetch:(...args)=>core.network.fetch(...args),capabilities,store:preferences},core.state,
   {post:(...args)=>post.post(...args),index:core.index,pending:core.pending,sync:core.sync,groups:core.groups,live:core.live},sessionUi,terminal,
   {...views,appConfirm:SessionUi.appConfirm,updatePick:Sidebar.updatePick,showMobileList:Shell.showMobileList,updateGroups:Sidebar.updateGroups,currentGroups:Sidebar.currentGroups,auditDetailRendered:core.diagnostics.auditDetailRendered,sessionTurn:(...args)=>status.sessionTurn(...args)})
 detailNotices=createDetailNotices(metadata)
 appearance=createAppearance({preferences,choices:Overlays.typography.choices,updateSettings:Settings.update,refreshTerminalPreferences:(...args)=>terminal.refreshTerminalPreferences(...args)})
 formulaRuntime=createFormulaRuntime()
 timeline=createTimeline({core,dom:()=>dom,sidebarView:()=>sidebarView})
 status=createStatus({terminal:()=>terminal,dom:()=>dom,composer:()=>composer,core,sidebarView:()=>sidebarView,status:()=>status,sessionUi:()=>sessionUi,conversationRenderer:()=>conversationRenderer,sidebarGestures:()=>sidebarGestures,bulk:()=>bulk,filters:()=>filters,capabilities,presentation})
 catalogMeta=createCatalogMeta({core,conversationRenderer:()=>conversationRenderer,sessionUi:()=>sessionUi})
 sidebarGestures=createSidebarGestures({core,sessionUi:()=>sessionUi,terminal:()=>terminal,sidebarView:()=>sidebarView,status:()=>status,bulk:()=>bulk,build:()=>build,metadata:()=>metadata,capabilities})
 sidebarView=createSidebarView({core,status:()=>status,bulk:()=>bulk,terminal:()=>terminal,sidebarGestures:()=>sidebarGestures,metadata:()=>metadata,timeline:()=>timeline,composer:()=>composer,filters:()=>filters,dom:()=>dom,capabilities,resources:()=>resources,sidebarResources:()=>sidebarResources})
 conversationRenderer=createConversationRenderer({core,sessionUi:()=>sessionUi,detailNotices:()=>detailNotices,syntaxRuntime:()=>syntaxRuntime,composer:()=>composer,terminal:()=>terminal,questionRuntime:()=>questionRuntime,pendingStage:()=>pendingStage,presentation})
 syntaxRuntime=createSyntaxRuntime({dom:()=>dom,conversationRenderer:()=>conversationRenderer,core})
 markdownRuntime=createMarkdownRuntime({dom:()=>dom,core,mediaRuntime:()=>mediaRuntime,sidebarGestures:()=>sidebarGestures,capabilities})
 mediaRuntime=createMediaRuntime({dom:()=>dom,core,capabilities})
 settingsRuntime=createSettingsRuntime({terminal:()=>terminal,core,sleep:()=>sleep,bulk:()=>bulk,appearance:()=>appearance,capabilities,sidebarResources:()=>sidebarResources})
 takeover=createTakeover({terminal:()=>terminal,core,dom:()=>dom,composer:()=>composer,presentation})
 pendingStage=createPendingStage({terminal:()=>terminal,core,conversationRenderer:()=>conversationRenderer,composer:()=>composer,pendingStage:()=>pendingStage,status:()=>status,sessionUi:()=>sessionUi,takeover:()=>takeover,sidebarView:()=>sidebarView,capabilities})
 consolePaste=createConsolePaste({settingsRuntime:()=>settingsRuntime,terminal:()=>terminal,composer:()=>composer,core,post:()=>post})
 sidebarResources=createSidebarResources(core,()=>terminal,()=>sessionUi)
 filters=createSessionFilters(core.state,core.index,core.nodes,core.groups,{sidebarMainMatches:(...args)=>sidebarView.sidebarMainMatches(...args),sidebarAgentItems:(...args)=>sidebarView.sidebarAgentItems(...args)},(...args)=>sidebarView.nestEdges(...args))
 launch=SessionUi.createLaunch(sharedUi)
 const shellBridge={get:preferences.get,set:preferences.set,openNewSession:()=>launch.openNewSessionDialog(),openSettings:Settings.open,openTrash:()=>sessionUi.openTrash(),openTransfers:()=>sessionUi.openTransferTasks(),
   selectView:value=>{core.state.sidebar.view=value;preferences.set('view',value);sidebarView.renderView();sidebarView.renderSide()},toggleNest:()=>{core.state.sidebar.nest=!core.state.sidebar.nest;preferences.set('nest',core.state.sidebar.nest);sidebarView.renderView();sidebarView.renderSide()},
   layoutSessionHead:()=>sessionUi.layoutSessionHead(),fitTerminal:()=>{if(terminal.state.term)terminal.fitTerm()},fitViewportTerminal:()=>terminal.fitTerm(),closeTerminal:()=>{if(!document.querySelector('#termpane').classList.contains('hidden'))terminal.closeTermPane(true)},selected:()=>!!core.state.selection.sel,
   freezeOverlay:()=>sessionUi.syncSessionFreezeOverlay(),stopNotice:()=>sessionUi.syncSessionStopNotice(),layoutTerminal:()=>{terminal.layoutTermPane();return true},scrollTerminal:()=>{try{terminal.currentTermViewObject()?.term?.scrollToBottom()}catch{}},positionTerminal:()=>terminal.positionTermViewport(terminal.currentTermViewObject()),cancelLongPress:()=>sidebarGestures.cancelLongPress(),closeItemMenu:()=>sidebarGestures.closeItemMenu(),resetItemClick:()=>sidebarGestures.resetItemClick(),refreshTerminalScale:terminal.refreshTerminalScale,updateScale:scale=>Settings.update({scale})}
 Shell.configure(shellBridge)
 let shellEnvironment
 function start(){
   startControlEvents()
   Terminal.mount(document.querySelector('#terminal-root'))
   Shell.mount(shellBridge)
   Shell.applyInterfaceScale();appearance.start()
   resources=Overlays.mountResources({fetch:(...args)=>core.network.fetch(...args),appUrl:environment.appUrl,selection:()=>core.state.catalog.sessions.find(row=>row.uid===core.state.selection.sel)||null})
   sleep=Overlays.mountSleep({store:preferences,storagePrefix:environment.STORAGE_PREFIX,network:core.network,settings:Settings})
   sidebarView.start();mediaRuntime.start();markdownRuntime.start();timeline.start();sidebarGestures.start();status.start();core.scroll.start();core.diagnostics.start();core.events.start();core.ticker.start();core.groups.start();core.nodes.start()
   sidebarResources.start();settingsRuntime.mountSettings()
   configureConversation({core,composer,sessionUi,metadata,questionRuntime,markdownRuntime,syntaxRuntime,formulaRuntime,mediaRuntime,dom})
   Search.renderOpts();bulk.renderPickBar();sidebarView.renderView();conversationRenderer.ensureConsolePlaceholder()
   composer.start();terminal.start();core.live.pollLive()
   setInterval(()=>{if(!core.events.ready)core.list.pollSessions()},8000)
   document.addEventListener('visibilitychange',()=>{if(!document.hidden)core.list.pollSessions()})
   build.start();shellEnvironment=createShellEnvironment(runtimePinia,{hub:environment.HUB_MODE,nodes:core.state.nodes,hostname:()=>build.state.hostname,fetch:(...args)=>core.network.fetch(...args),appUrl:environment.appUrl,audit:(...args)=>core.audit.browserAuditEvent(...args),alert:SessionUi.appAlert});mountRuntimeNotices(build,shellEnvironment,SessionUi.floatStack);shellEnvironment.start()
   startNetworkLifecycle({network:core.network,selection:core.state.selection,list:core.list,sync:core.sync,events:core.events,checkServerBuild:build.checkServerBuild,pollLive:(...args)=>core.live.pollLive(...args),flushBrowserAudit:core.audit.flushBrowserAudit,showLoginExpired:build.showLoginExpired})
   document.addEventListener('click',event=>{if(event.target.closest('[data-report-bug]'))launch.openBugReportDialog()})
   document.addEventListener('keydown',event=>{if(event.key!=='Escape')return;if(core.groups.escapeMenu())return;if(!sidebarGestures.menuState.hidden)return sidebarGestures.closeItemMenu();if(core.state.sidebar.nestAttach)return sidebarView.setNestAttach('');if(core.state.sidebar.picking)return bulk.setPicking(false);document.querySelector('#q').blur()})
   if(capabilities.config.configuration_error)presentation.backendNotice='能力配置无效，请检查服务配置。'
   Shell.syncPageReload()
   restoreSession(core,terminal,sidebarView)
 }
 return {core,terminal,composer,conversation:Conversation,sessionUi,launch,metadata,bulk,post,build,appearance,get sleep(){return sleep},get resources(){return resources},filters,
   dom,timeline,status,catalogMeta,sidebarGestures,sidebarView,conversationRenderer,syntaxRuntime,formulaRuntime,questionRuntime,markdownRuntime,mediaRuntime,settingsRuntime,takeover,pendingStage,consolePaste,sidebarResources,detailNotices,overlays:Overlays,shell:Shell,search:Search,settings:Settings,machines:Machines,capabilities,cli:Cli,format:Format,viewKey,GridTerm,get shellEnvironment(){return shellEnvironment},start}
}
function restoreSession(core,terminal,sidebarView){
 const {selection,catalog}=core.state,{MOBILE,DEEP_SID}=core.environment,preferences=core.preferences
 const leave=()=>{if(selection.sel||!document.body.classList.contains('mobile-detail'))return;Shell.leaveBootDetail();showDetailState('empty','从左侧选择一个会话');Shell.layoutHeader()}
 addEventListener('popstate',()=>{const route=core.routing.routeSession(new URL(location.href).searchParams.get('sid')||'');if(route)core.open.openSession(route.uid,route.agent,{exact:true,historyMode:'none'})})
 if(document.body.classList.contains('mobile-detail')&&!selection.sel)showDetailState('spin','正在读取会话…')
 core.list.loadSessions(false).then(async ok=>{
  if(!ok)return leave();const route=core.routing.routeSession(DEEP_SID);if(route){core.open.openSession(route.uid,route.agent,{exact:true,historyMode:'replace'});return}
  const last=preferences.get('sel',null),savedAgent=preferences.get('agent',null),restore=!MOBILE.matches||preferences.get('mobilePage','list')==='detail'
  if(restore&&last&&catalog.sessions.some(s=>s.uid===last)){core.open.openSession(last,savedAgent?.uid===last?savedAgent.id:null)}
  else if(restore&&last?.startsWith('tmux:')){let request=terminal.state.listRequest||terminal.loadTermList();do{await request;request=terminal.state.listRequest}while(request);if(selection.sel||preferences.get('sel',null)!==last||(MOBILE.matches&&preferences.get('mobilePage','list')!=='detail'))return;const pending=terminal.state.pending.find(row=>pendingUid(row.name)===last)||core.pending.pendingTmuxSessions().find(row=>row.uid===last);if(pending)await terminal.openPendingSession(pending);else leave()}
  else leave()
 })
}

function configureConversation({core,composer,sessionUi,metadata,questionRuntime,markdownRuntime,syntaxRuntime,formulaRuntime,mediaRuntime,dom}){
Conversation.configure({
  md:markdownRuntime.md, head:sessionUi.head, renderFormulae:formulaRuntime.renderFormulae, paintSyntax:syntaxRuntime.paintSyntax, clearSyntaxPaint:syntaxRuntime.clearSyntaxPaint, paintToolOutputDiff:syntaxRuntime.paintToolOutputDiff,
  retryableReadFailure:retryableReadFailure, retryMigrationRead:core.sync.retryMigrationRead,
  safeMediaSrc:mediaRuntime.safeMediaSrc, lazyMediaEnabled:mediaRuntime.lazyMediaEnabled, mediaContinuationEnabled:mediaRuntime.mediaContinuationEnabled, mediaMoreInfo:mediaRuntime.mediaMoreInfo, diagnoseMedia:mediaRuntime.diagnoseMedia,
  createView: mediaRuntime.captureView,
  forgetMediaDiagnostic:mediaRuntime.forgetMediaDiagnostic,
  reloadMediaOwned: (view, notice) => mediaRuntime.reloadMediaSession(view, {}, {set textContent(value) {notice(value);}}),
  loadMedia: (cursor, button) => mediaRuntime.loadMediaContinuation(core.state.selection.sel,core.state.selection.agent,cursor,button),
  loadHistory: (info,button) => core.history.historyPagesEnabled() ? core.history.loadHistoryPage(info.uid,info.agent,button) : core.history.loadFullHistory(info.uid,info.agent,button),
  reloadHistory: (info,button) => core.history.reloadHistoryWindow(info.uid,info.agent,button),
  sticking: () => core.scroll.state.stick, live: uid => core.state.live.live.has(uid), settle:core.scroll.settle,
  mutateKeepingMessageAnchor:core.scroll.mutateKeepingMessageAnchor, jumpWithinConversation:core.scroll.jumpWithinConversation, uiIcon:dom.uiIcon,
  sessiondockCli:sessiondockCli, answer: (...args) => composer.answerCliQuestion(...args),
  cancel: (...args) => composer.cancelCliQuestion(...args),
  answerCliQuestionForm: (...args) => composer.answerCliQuestionForm(...args),
  revealNativeTerminal: uid => composer.revealNativeTerminal(uid),
  dismissQueuedSend: (...args) => composer.dismissQueuedSend(...args),
  pinTimeline:metadata.pinTimeline, pinTarget: m => {
    if (!metadata.timelinePinEnabled() || m.role !== 'user' || m.turn_id == null || core.state.selection.agent) return null;
    const session = core.state.catalog.sessions.find(s => s.uid === core.state.selection.sel) || core.cache.cache.get(viewKey(core.state.selection.sel,null))?.meta;
    return session?.source === 'claude' ? String(m.turn_id) : null;
  },
  questionDraft: (m,rows) => {
    const key=m.uid && m.call_id ? `${m.uid}\0${m.call_id}` : '';
    let value=key ? questionRuntime.questionFormDrafts.get(key) : null;
    if (!Array.isArray(value) || value.length!==rows.length || !value.every((n,i)=>n===null || (Number.isInteger(n) && !!rows[i]?.options?.[n]))) {value=Array(rows.length).fill(null);if(key)questionRuntime.questionFormDrafts.set(key,value);}
    return value;
  },
  screenText: m => questionRuntime.screenMenuTextDrafts.get(`${composer.composerDraftOwner(m.uid)}\0${m.id}`),
  saveScreenText: (m,value) => {
    const owner=composer.composerDraftOwner(m.uid),key=`${owner}\0${m.id}`;
    for(const saved of questionRuntime.screenMenuTextDrafts.keys())if(saved.startsWith(`${owner}\0`) && saved!==key)questionRuntime.screenMenuTextDrafts.delete(saved);
    questionRuntime.screenMenuTextDrafts.set(key,value);
  },
  hasTerm:Search.hasTerm, searchCanOpen: () => core.state.search.autoOpen < markdownRuntime.AUTO_OPEN_MAX,
  claimSearchOpen: () => {if (core.state.search.autoOpen >= markdownRuntime.AUTO_OPEN_MAX) return false;core.state.search.autoOpen++;return true;},
  searchMessage: text => {const found=Search.hasTerm(text),open=found && core.state.search.autoOpen < markdownRuntime.AUTO_OPEN_MAX;if(open)core.state.search.autoOpen++;return {found,open};},
  searchAsync: (text,apply) => {
    if (!core.state.search.term || !core.state.search.opts.regex || Search.hasTerm(text)) return;
    const generation=Search.generation();
    Search.regexMatches(text).then(ranges => {if(generation===Search.generation() && ranges.matched)apply();});
  },
  searchProcessAsync: (items,apply) => {
    if (!core.state.search.term || !core.state.search.opts.regex || items.some(m => Conversation.planning.SEARCH_ROLES.has(m.role) && Search.hasTerm(m.text))) return;
    const generation=Search.generation();
    Promise.all(items.filter(m => Conversation.planning.SEARCH_ROLES.has(m.role)).map(m => Search.regexMatches(m.text))).then(results => {if(generation===Search.generation() && results.some(r=>r.matched))apply();});
  },
  markInner: root => {if(core.state.search.term){Search.markMatches(root);Search.updateMatchNav();}},
});
Conversation.pages.configurePages(mediaRuntime.mediaMoreInfo)
}
