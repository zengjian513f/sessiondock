import type { Pinia } from 'pinia'
import type { WorkspaceState } from './workspace'
import type { CacheService } from './cache.js'
import type { SessionIndexService } from './session-index.js'
import type { SessionSyncService } from './session-sync.js'
export interface MetadataPresentation {
  renderChips(): void
  renderSide(): void
  showSessionCount(count?: number): void
  renderSessionAction(meta: any): void
  renderForkChainMenu(): void
  appAlert(message: string): Promise<void>
  paintStarButton(button: HTMLButtonElement | null, starred: boolean, busy: boolean): void
  status: {textContent: string; classList: {add(name: string): void; remove(name: string): void}}
}
export interface MetadataService {
  sessionStarred(uid: string): boolean
  applySessionStar(uid: string, starred: boolean, starredAt?: string | null): void
  applyForkParentVisibility(uid: string, visible: boolean): void
  setForkParentVisibility(uids: string[], visible: boolean, button?: HTMLButtonElement | null): Promise<any>
  refreshStarPresentation(uid: string): void
  toggleSessionStar(uid: string): Promise<void>
  timelinePinEnabled(): boolean
  timelinePinFailed(message: string): void
  pinTimeline(uid: string, target: string | null): Promise<boolean>
}
export function createMetadataService(pinia: Pinia, environment: {fetch: typeof fetch; appUrl(path: string): string; capabilities: {config: {backend?: unknown; timeline_pin?: boolean}}},
  state: WorkspaceState, cache: CacheService, index: SessionIndexService, nodes: {renderNodes(): void},
  sync: Pick<SessionSyncService,'syncSession'>, presentation: MetadataPresentation): MetadataService
