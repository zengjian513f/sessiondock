import type { Pinia } from 'pinia'
import type { NodeRecord } from '../../stores/runtime/nodes'
import type { NetworkService } from './network.js'

export interface NodeEnvironment {
  HUB_MODE: boolean
  STORAGE_PREFIX: string
  capabilities: {config: {[name: string]: any}; stored(key: string, prefix: string): string | null; allows(name: string): boolean}
  store: {get(key: string, fallback: any): any; set(key: string, value: any): void}
  SOURCES: {[source: string]: {name: string}}
  fetch: NetworkService['fetch']
  appUrl(path: string): string
}
export interface NodeSessions {
  catalog: {sessions: any[]}
  selection: {sel: string | null}
  sessionHidden(row: any): boolean
}
export interface NodeTerminal {
  state: any
  vendors: {Terminal: any; FitAddon: any}
  ensureAssets?: (...args: any[]) => Promise<void>
  takeover(uid: string, button: HTMLButtonElement): Promise<void>
  sessionRecordingReplayable(uid: string): boolean
  sessionTermMeta(uid: string): any
  linkedTermSession(uid: string, options: {followReplacement: boolean}): any
  pendingPhase(row: any): string
  toggleLinkedTermSession(uid: string): Promise<void>
  renderTakeoverBtn(): void
}
export interface NodeLaunch {
  canCompleteCwd(value: string): boolean
  closeCwdPicker(): void
  cwdCompletion: {common: any[]}
  commonSessionDirs(): any[]
  renderCommonCwdOptions(): void
}
export interface NodeSidebar {
  ensureSidebarVue(): void
  updateNodes(rows: any[]): void
}
export interface NodePresentation {
  consoleButton: {unavailable: boolean; title: string; label: string}
  shortCwd(path: string, length?: number): string
  esc(value: any): string
  uiIcon(name: string): string
  appAlert(message: string): Promise<void>
}
export interface NodeService {
  ConsoleUI: {errors: Map<string, string>; busy: Set<string>}
  nodeOf(uid: string | null): string
  nodeSelected(row: any): boolean
  selectedNodeIds(): string[]
  nodeDirectory(row: any, length?: number): string
  nodeColor(name: string): string
  newNodeId(): string
  newDirsKey(): string
  newNodeCapabilities(): any
  sessionTerminalEnabled(uid: string): boolean
  consoleUnavailableReason(uid: string | null, agent?: string | null, lastError?: boolean): string
  showConsoleToast(reason: string): void
  paintConsoleAvailability(button: HTMLButtonElement, uid: string, agent?: string | null): void
  applyNodeState(data: any, context?: string): void
  nodeClock(ts: number): string
  nodeAgo(ts: number): string
  nodeOfflineReason(node: NodeRecord): string
  nodeRequestFailures(node: NodeRecord): string[]
  nodeChipReason(node: NodeRecord): string
  nodeAbbrs(nodes: NodeRecord[]): Map<string, string>
  renderNodes(): void
  loadNodes(): Promise<void>
  start(): void
}
export function createNodeService(pinia: Pinia, environment: NodeEnvironment, sessions: NodeSessions,
  terminal: NodeTerminal, launch: NodeLaunch, sidebar: NodeSidebar, presentation: NodePresentation): NodeService
