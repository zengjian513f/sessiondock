import type { WorkspaceState } from './workspace'
import type { CacheService } from './cache.js'
import type { NetworkService } from './network.js'
import type { ReadOptions } from './messages.js'
export interface SessionSyncService {
  timing: {syncStallMs: number}
  syncingViews: Map<string, Promise<number>>
  migrationReadFailures: Map<string, any>
  migrationReadRetries: Map<string, Promise<boolean>>
  migrationReadProbes: Map<string, Promise<boolean>>
  migrationReadPaused(uid: string, agent?: string | null): boolean
  renderMigrationReadFailure(uid: string, agent?: string | null): void
  reportMigrationReadFailure(uid: string, agent: string | null, error: any): any
  reportReadFailure(uid: string, agent: string | null, error: any): any
  retryMigrationRead(uid: string, agent?: string | null): Promise<boolean>
  pauseMigrationWatch(stream: EventSource, uid: string, agent: string | null, error?: any): boolean
  probeWatchRejection(uid: string, agent: string | null): Promise<boolean>
  syncSession(uid: string, agent?: string | null): Promise<number>
  scheduleDiffRecovery(uid: string, agent?: string | null): void
  watchSession(uid: string, agent?: string | null): void
  closeWatch(): void
  readonly watching: EventSource | null
  readonly watchedUid: string | null
}
export interface SyncRendering {
  applyDiff(uid: string, data: any, bytes?: number, agent?: string | null): Promise<number>
  renderSession(meta: any, messages: any[], activity: any, options?: any): Promise<void>
}
export interface SyncAudit {
  browserAuditEvent(event: string, data?: any, content?: any, fields?: any): void
  browserStateSnapshot(reason: string): {content: any}
  scheduleBrowserSnapshot(reason: string): void
}
export function createSessionSync(environment: {capabilities: {config: {backend?: unknown}}; network: NetworkService; appUrl(path: string): string; pageId: string},
  state: WorkspaceState, cache: CacheService,
  reader: {fetchMessages(uid: string, options?: ReadOptions): Promise<{data: any; bytes: number}>},
  rendering: SyncRendering, audit: SyncAudit): SessionSyncService
