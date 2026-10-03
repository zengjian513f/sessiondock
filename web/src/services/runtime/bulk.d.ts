import type { Pinia } from 'pinia'
import type { WorkspaceState } from './workspace'
import type { SessionIndexService } from './session-index.js'
import type { PendingSessionsService } from './pending-sessions.js'
import type { SessionSyncService } from './session-sync.js'
import type { GroupsService } from './groups.js'
import type { usePickOperationsStore } from '../../stores/runtime/picking'
export interface BulkSessionUi {
  sessionStopCapable(): boolean
  sessionStoppable(uid: string): boolean
  showSessionStopNotice(message: string): void
  trashLocationNote(): string
  opencodeDeleteNote(): string
  trashCapable(): boolean
  confirmForceDelete(count: number, detail: any): Promise<boolean>
  requestSessionStop(target: any): Promise<any>
  openTrash(): void
}
export interface BulkPresentation {
  appAlert(message: string): Promise<void>
  appConfirm(message: string): Promise<boolean>
  renderSide(): void
  renderChips(): void
  updatePick(value: any): void
  ensureSidebarVue(): void
  showMobileList(): void
  visible(): any[]
  ensureConsolePlaceholder(): void
  status: {textContent: string}
  refreshSidebarRows(uid: string): void
  updateGroups(groups: any[]): void
  currentGroups(): any[]
  auditDetailRendered(reason: string): void
  sessionTurn(uid: string): string
  paintLive(): void
}
export interface BulkOperations {
  sessionPickable(session: any): boolean
  state: ReturnType<typeof usePickOperationsStore>
  gesture: {drag: any; swallowClick: boolean}
  sessionStopConcurrency(): number
  pickedStopTargets(): any[]
  syncPickedSessions(): Set<string>
  setPicking(on: boolean): void
  toggleSessionPick(uid: string): void
  toggleGroupPick(uids: string[], group?: any): void
  pickDragRows(): HTMLElement[]
  pickDragTo(row: HTMLElement): void
  pickDragAt(y: number): void
  pickDragScroll(): void
  endPickDrag(): void
  sidebarGestureMousedown1(event: MouseEvent): void
  sidebarGestureSelectstart1(event: Event): void
  sidebarGestureClick1(event: MouseEvent): void
  pickAllVisible(): void
  renderPickBar(): void
  deleteSessions(uids: string[], button?: HTMLButtonElement | null): Promise<any>
  deletePickedSessions(): Promise<void>
  stopPickedSessions(): Promise<void>
  pickedNestable(): string[]
}
export function createBulkOperations(pinia: Pinia, environment: {capabilities: {config: any; allows(name: string): boolean}; HUB_MODE: boolean; fetch: typeof fetch; appUrl(path: string): string;
  store: {get(key: string, fallback: any): any; set(key: string, value: any): void}}, state: WorkspaceState,
  services: {post(path: string, body: any): Promise<any>;
    index: SessionIndexService; pending: PendingSessionsService; sync: SessionSyncService; groups: GroupsService;
    live: {refreshLive(force?: boolean): Promise<void>}}, sessionUi: BulkSessionUi,
  terminal: {state: any; discardPendingSession(row: any): Promise<any>; loadTermList(): Promise<any>},
  presentation: BulkPresentation): BulkOperations
