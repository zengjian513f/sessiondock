import type { CacheService } from './cache.js'
import type { ReadOptions } from './messages.js'
export interface UnreadService {
  readonly sidebarSyncing:Set<string>
  readonly sidebarPendingCursors:Map<string,any>
  readonly unreadBatchUnsupported:Set<string>
  cursorViews(sessions: any[]): any[]
  cleanCursor(value: any): {end: number; head: string; anchor: string} | null
  seedSidebarCursors(sessions: any[]): void
  fetchUnreadSummary(uid: string, options: any): Promise<any>
  flushUnreadBatch(node: string, batch: any[]): Promise<void>
  syncSidebarView(row: any, base: any, latest: any, attempt?: number): Promise<void>
  syncSidebarUpdates(sessions: any[]): void
}
export function createUnreadService(environment: {
  capabilities: {config: {backend?: unknown; unread_batch?: boolean}}
  fetch: typeof fetch
  appUrl(path: string): string
  MOBILE: MediaQueryList
  HUB_MODE: boolean
  nodeOf(uid: string): string
  timing: {syncStallMs: number}
}, selection: {sel: string | null; agent: string | null}, state: {cursors: Map<string, any>}, cache: CacheService,
  reader: {fetchMessages(uid: string, options?: ReadOptions): Promise<{data: any; bytes: number}>},
  status: {unreadRow(uid: string): {count: number}; addUnread(uid: string, count: number): void}): UnreadService
