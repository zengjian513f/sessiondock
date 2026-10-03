import type { WorkspaceState } from './workspace'
import type { CacheService } from './cache.js'
import type { SessionIndexService } from './session-index.js'
import type { SessionSyncService } from './session-sync.js'
export interface DiffPresentation {
  sealTurnTail(box: HTMLElement, entry: any): boolean
  renderConversationTail(activity: any, uid: string): void
  renderSessionAction(meta: any): void
  renderSide(): void
  head(meta: any, count: number): HTMLElement
  layoutSessionHead(): void
  paintTurn(uid: string): void
  renderSession(meta: any, messages: any[], activity: any, options?: any): Promise<void>
  addUnread(uid: string, count: number): void
  appendMessages(box: HTMLElement, messages: any[], before: any, options?: any): HTMLElement[]
  markMatches(box: HTMLElement): void
  updateMatchNav(): void
  sidebarTextSelectionProtected(): boolean
  refreshSidebarRows(uid: string): void
}
export interface DiffService {
  applyCoveredActivity(uid: string, agent: string | null, entry: any, data: any): void
  applyMigrationMeta(uid: string, agent: string | null, entry: any, meta: any): void
  applyDiff(uid: string, data: any, bytes?: number, agent?: string | null): Promise<number>
  applyDiffPacket(uid: string, data: any, bytes?: number, agent?: string | null): Promise<number>
}
export function createDiffService(environment: {capabilities: {config: {backend?: unknown}}; MOBILE: MediaQueryList},
  state: WorkspaceState, cache: CacheService, index: Pick<SessionIndexService, 'indexedSessions'>,
  sync: Pick<SessionSyncService, 'migrationReadPaused' | 'scheduleDiffRecovery'>,
  conversation: {messageIndex(messages: any[]): {questions: Set<string>}; markInterruptedTurn(messages: any[], activity: any): void},
  composer: {applyCliState(uid: string, cli: any): void}, terminal: {revealConversationForPrompt(uid: string, prompt: any): void},
  presentation: DiffPresentation): DiffService
