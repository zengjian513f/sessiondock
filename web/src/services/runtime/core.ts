import type { Pinia } from 'pinia'
import { createEnvironment } from './environment.js'
import { createPreferences } from './preferences'
import { createWorkspaceState } from './workspace'
import { createNetworkService } from './network.js'
import { createAuditService } from './audit.js'
import { createMessageReader } from './messages.js'
import { createHistoryReader } from './history-reader.js'
import { createMessageCache } from './cache.js'
import { createSessionIndex } from './session-index.js'
import { createPendingSessions } from './pending-sessions.js'
import { createNodeService } from './nodes.js'
import type { NodeService, NodeTerminal, NodeLaunch, NodeSidebar, NodePresentation } from './nodes.js'
import { createGroupsService } from './groups.js'
import type { GroupsSidebar } from './groups.js'
import { createUnreadStatus } from './unread-status'
import { createUnreadService } from './unread.js'
import { createSessionSync } from './session-sync.js'
import type { SyncRendering } from './session-sync.js'
import { createDiffService } from './diff.js'
import type { DiffPresentation, DiffService } from './diff.js'
import { createSessionList } from './session-list.js'
import type { ListPresentation, SessionListService } from './session-list.js'
import { createLiveService } from './live.js'
import { createUiEvents } from './ui-events.js'
import { createSyncTicker } from './sync-ticker.js'
import { createSessionRouting } from '../../domain/runtime/routing.js'
import { SOURCES } from '../../domain/runtime/sources'
import { useNodeStore, useConsoleStore } from '../../stores/runtime/nodes'
import type { StorageCapabilities } from './preferences'
import { createSessionOpen } from './session-open.js'
import type { OpenPresentation } from './session-open.js'
import { createConversationScroll } from './scroll.js'
import { createHistoryController } from './history.js'
import { createDiagnostics } from './diagnostics.js'

export interface RuntimeCapabilities extends StorageCapabilities {
  config: {[flag: string]: any}
  allows(name: string): boolean
}
export interface RuntimeTerminal extends NodeTerminal {
  terminalListUncertain(uid: string): boolean
  loadTermList(): Promise<any>
  closeTermPane(preserveView?: boolean): void
  revealConversationForPrompt(uid: string, prompt: any): void
}
export interface RuntimeComposer {
  drafts: Map<string, any>
  applyCliState(uid: string, cli: any): void
  hide(): void
  migrateComposerDraft(from: string, to: string): void
}
export interface RuntimeCoreDependencies {
  initialEnvironment?: ReturnType<typeof createEnvironment>
  initialPreferences?: ReturnType<typeof createPreferences>
  capabilities: RuntimeCapabilities
  terminal: RuntimeTerminal
  composer: RuntimeComposer
  launch: NodeLaunch
  sidebar: NodeSidebar & GroupsSidebar
  itemMenu: {readonly uid: string; close(): void; setGroupExpanded(expanded:boolean):void}
  presentation: NodePresentation & Omit<ListPresentation, 'followSelectedFork'> & Omit<DiffPresentation, 'addUnread'> & Omit<OpenPresentation, 'clearUnread' | 'browserAuditEvent' | 'auditDetailRendered'> &
    Omit<SyncRendering, 'applyDiff'> & {renderTakeoverBtn(): void}
  conversation: {messageIndex(messages: any[]): {questions: Set<string>}; markInterruptedTurn(messages: any[], activity: any): void;
    flushPendingTurnSeal(box: HTMLElement): void; validateHistoryPage(data: any, partial: any, cursor: string): any}
  rendering: {cancel(): void; clearSyntax(): void; clearFormulae(): void}
  el(tag: string, className?: string, text?: string): HTMLElement
}
// Compose named runtime services without a global state/operation registry.
// Rendering remains an explicit boundary; no component callback owns transport.
export function createRuntimeCore(pinia: Pinia, deps: RuntimeCoreDependencies) {
  let nodes: NodeService, list: SessionListService, diff: DiffService
  const environment = deps.initialEnvironment || createEnvironment(deps.capabilities, () => nodes.selectedNodeIds(), () => nodes.newNodeId())
  const preferences = deps.initialPreferences || createPreferences(deps.capabilities, environment.STORAGE_PREFIX)
  const state = createWorkspaceState(pinia, preferences, deps.capabilities)
  const network = createNetworkService(pinia, deps.capabilities)
  const common = {...environment, capabilities:deps.capabilities, network, fetch:(...args:Parameters<typeof fetch>)=>network.fetch(...args),
    pageId:environment.AUDIT_PAGE_ID, buildId:environment.BUILD_ID, store:preferences, SOURCES}
  const audit = createAuditService({...common, selected:() => state.selection.sel})
  const auditEnvironment = {...common, browserAuditEvent:(...args:Parameters<typeof audit.browserAuditEvent>)=>audit.browserAuditEvent(...args)}
  const reader = createMessageReader(auditEnvironment)
  const historyReader = createHistoryReader(auditEnvironment)
  const cache = createMessageCache(pinia,preferences,state.selection,state.live,() => deps.terminal.state?.list || [])
  const diagnostics = createDiagnostics(common,state.selection,deps.terminal,cache,audit)
  const index = createSessionIndex(state,() => pending.pendingTmuxSessions())
  const pending = createPendingSessions(deps.capabilities,SOURCES,deps.terminal,deps.composer.drafts,() => index.indexedSessions())
  nodes = createNodeService(pinia,common,{catalog:state.catalog,selection:state.selection,sessionHidden:index.sessionHidden},
    deps.terminal,deps.launch,deps.sidebar,deps.presentation)
  const groups = createGroupsService(pinia,common,{...state,cache:cache.cache,indexedSessions:index.indexedSessions,
    loadSessions:force => list.loadSessions(force)},deps.sidebar,deps.itemMenu)
  const status = createUnreadStatus(state.unread,preferences,uid => {
    if (document.querySelector(`.item[data-uid="${CSS.escape(uid)}"]`)) deps.presentation.refreshSidebarRows(uid)
  })
  const sync = createSessionSync(common,state,cache,reader,{
    applyDiff:(...args) => diff.applyDiff(...args),renderSession:deps.presentation.renderSession,
  },{...diagnostics,browserAuditEvent:(...args:Parameters<typeof audit.browserAuditEvent>)=>audit.browserAuditEvent(...args)})
  diff = createDiffService(common,state,cache,index,sync,deps.conversation,deps.composer,deps.terminal,
    {...deps.presentation,addUnread:status.addUnread})
  const unread = createUnreadService({...common,nodeOf:nodes.nodeOf,timing:sync.timing},state.selection,state.unread,cache,reader,status)
  const open = createSessionOpen(common,state,cache,reader,sync,index,deps.terminal,deps.composer,deps.rendering,
    {...deps.presentation,browserAuditEvent:(...args:Parameters<typeof audit.browserAuditEvent>)=>audit.browserAuditEvent(...args),auditDetailRendered:diagnostics.auditDetailRendered,clearUnread:status.clearUnread})
  const scroll = createConversationScroll(pinia,deps.conversation.flushPendingTurnSeal)
  const history = createHistoryController(common,state,cache,reader,historyReader,open,Object.assign(sync,{applyDiff:diff.applyDiff}),
    scroll,deps.conversation.validateHistoryPage,{...deps.presentation,el:deps.el})
  list = createSessionList(common,state,nodes,index,unread,{...deps.presentation,followSelectedFork:open.followSelectedFork})
  const live = createLiveService(common,state.catalog,state.live,nodes,deps.terminal,deps.presentation,cache.trimCache)
  const events = createUiEvents(common,state.catalog,list,deps.terminal,live.refreshLive,unread.syncSidebarUpdates)
  const ticker = createSyncTicker(network,state.selection,state.live,sync)
  const routing = createSessionRouting(state.catalog,environment.deepNode)
  return {environment,preferences,state:{...state,nodes:useNodeStore(pinia),console:useConsoleStore(pinia)},
    network,audit,diagnostics,reader,historyReader,cache,index,pending,nodes,groups,status,sync,diff,unread,open,scroll,history,list,live,events,ticker,routing}
}
export type RuntimeCore = ReturnType<typeof createRuntimeCore>
