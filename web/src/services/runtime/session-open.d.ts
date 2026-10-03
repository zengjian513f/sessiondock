import type { WorkspaceState } from './workspace'
import type { CacheService } from './cache.js'
import type { ReadOptions } from './messages.js'
import type { SessionSyncService } from './session-sync.js'
import type { SessionIndexService } from './session-index.js'
export interface OpenOptions {exact?: boolean; historyMode?: 'push' | 'replace' | 'none'; follow?: boolean}
export interface OpenPresentation {
  browserAuditEvent(event: string, data?: any, content?: any, fields?: any): void
  showMobileDetail(): void
  syncSessionStopNotice(): void
  clearUnread(uid: string): void
  revealSessionInSidebar(uid: string, agent: string | null): void
  paintSidebarSelection(uid: string, agent: string | null): void
  renderSession(meta: any, messages: any[], activity: any, options?: any): Promise<void>
  ensureConsolePlaceholder(): void
  auditDetailRendered(reason: string, detail?: any): void
  progress(done: number, total: number, label: string, detail?: any): void
  progressDone(): void
  esc(value: any): string
}
export interface SessionOpenService {
  requests: {inflight: AbortController | null}
  openRetries: Map<string, number>
  followContinuedSession(uid: string): string
  scheduleOpenRetry(uid: string, agent: string | null, controller: AbortController): void
  followSelectedFork(): Promise<boolean>
  sessionUrl(uid: string, agent?: string | null): URL | null
  updateSessionUrl(uid: string, agent: string | null, mode: 'push' | 'replace' | 'none'): void
  openSession(uid: string, agent?: string | null, options?: OpenOptions): Promise<void>
}
export function createSessionOpen(environment: {capabilities: {config: {backend?: unknown}}; MOBILE: MediaQueryList; store: {get(key: string, fallback: any): any; set(key: string, value: any): void}},
  state: WorkspaceState, cache: CacheService,
  reader: {fetchMessages(uid: string, options?: ReadOptions): Promise<{data: any; bytes: number}>},
  sync: SessionSyncService, index: SessionIndexService,
  terminal: {state: any; closeTermPane(preserveView?: boolean): void},
  composer: {hide(): void; migrateComposerDraft(from: string, to: string): void},
  rendering: {cancel(): void; clearSyntax(): void; clearFormulae(): void}, presentation: OpenPresentation): SessionOpenService
