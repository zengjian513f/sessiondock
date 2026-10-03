import type { NetworkService } from './network.js'
import type { SessionListService } from './session-list.js'
export interface UiEventsService {
  queueUiChange(change: any): void
  applyUiChanges(): Promise<void>
  closeUiEvents(): void
  startUiEvents(): void
  start(): void
  readonly connection: EventSource | null
  readonly retry: number
  readonly applying: boolean
  readonly ready: boolean
}
export function createUiEvents(environment: {capabilities: {config: {ui_events?: boolean}}; network: NetworkService; appUrl(path: string): string},
  catalog: {sessions: any[]}, list: SessionListService,
  terminal: {loadTermList(): Promise<any>; state: {listError: string}}, refreshLive: () => Promise<any>,
  syncSidebarUpdates: (rows: any[]) => void): UiEventsService
