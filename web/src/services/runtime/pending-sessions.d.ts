export interface PendingSessionsService {
  pendingDraftStartedAt(uid: string, session: any): number
  pendingTmuxSessions(): any[]
  pendingNativeKey(row: any): string
  forgetDeletedReceipts(rows: any[]): void
}
export function createPendingSessions(capabilities: {config: {backend?: unknown}},
  sources: {[source: string]: {name: string}}, terminal: {state: {pending: any[]}; terminalListUncertain(uid: string): boolean},
  drafts: Map<string, any>, indexedSessions: () => {byNative: Map<string, any>}): PendingSessionsService
