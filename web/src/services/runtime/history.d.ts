import type { WorkspaceState } from './workspace'
import type { CacheService } from './cache.js'
import type { ReadOptions } from './messages.js'
import type { SessionOpenService } from './session-open.js'
import type { SessionSyncService } from './session-sync.js'
import type { ConversationScroll } from './scroll.js'
export interface HistoryController {
  timing: {chain: boolean}
  historyPageRequests: Map<string, any>
  historyPagesEnabled(): boolean
  currentHistoryPage(request: any): boolean
  historyPageFailure(request: any, button: HTMLButtonElement, error: any): void
  restoreHistoryPageScroll(top: number, scrollTop: number, entry: any): void
  loadHistoryPage(uid: string, agent: string | null, button: HTMLButtonElement): Promise<void>
  currentHistoryPageEntry(request: any): boolean
  reloadHistoryWindow(uid: string, agent: string | null, button: HTMLButtonElement): Promise<void>
  loadFullHistory(uid: string, agent: string | null, button: HTMLButtonElement): Promise<void>
}
export function createHistoryController(environment: {capabilities: {config: {backend?: unknown; history_pages?: boolean}}},
  state: WorkspaceState, cache: CacheService,
  reader: {fetchMessages(uid: string, options?: ReadOptions): Promise<{data: any; bytes: number}>},
  historyReader: {fetchHistoryPage(uid: string, agent: string | null, cursor: string, signal: AbortSignal, partial: any): Promise<{data: any; bytes: number}>},
  sessionOpen: Pick<SessionOpenService,'requests'>,
  sync: SessionSyncService & {applyDiff(uid: string, data: any, bytes: number, agent: string | null): Promise<number>},
  scroll: ConversationScroll, validateHistoryPage: (data: any, partial: any, cursor: string) => any,
  presentation: {renderSession(meta: any, messages: any[], activity: any, options?: any): Promise<void>;
    progress(done: number, total: number, label: string, detail?: any): void; progressDone(): void;
    el(tag: string, className?: string, text?: string): HTMLElement}): HistoryController
