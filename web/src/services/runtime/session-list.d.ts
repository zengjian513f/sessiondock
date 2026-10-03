import type { WorkspaceState } from './workspace'
import type { NetworkService } from './network.js'
import type { NodeService } from './nodes.js'
import type { SessionIndexService } from './session-index.js'
import type { UnreadService } from './unread.js'
export interface ListPresentation {
  status: {textContent: string; classList: {add(name: string): void; remove(name: string): void}}
  ensureSidebarVue(): void
  loadFailed(retry: () => void): void
  refreshSessionMeta(): void
  renderChips(): void
  renderSide(): void
  showSessionCount(count: number): void
  followSelectedFork(): Promise<any>
  patchSide(list: any[]): boolean
  visible(): any[]
  paintLive(): void
}
export interface SessionListService {
  loadSessions(force?: boolean): Promise<boolean>
  pollSessions(): Promise<any>
  runSessionPoll(): Promise<boolean>
  pause(): void
  readonly polling: Promise<any> | null
}
export function createSessionList(environment: {network: NetworkService; fetch: typeof fetch; appUrl(path: string): string},
  state: WorkspaceState, nodes: Pick<NodeService, 'applyNodeState' | 'renderNodes'>,
  index: Pick<SessionIndexService, 'hiddenForkParent' | 'sidebarSessions'>,
  unread: Pick<UnreadService, 'seedSidebarCursors' | 'syncSidebarUpdates'>,
  presentation: ListPresentation): SessionListService
