export interface SessionIndexService {
  indexedSessions(): {byUid: Map<string, any>; byNative: Map<string, any>; forkChildren: Map<string, any[]>}
  sessionContinued(session: any): boolean
  hiddenForkParent(session: any): boolean
  sessionHidden(session: any): boolean
  forkAncestors(session: any): {sid: string; row: any}[]
  forkChildren(session: any): any[]
  forkLeaf(session: any): any
  forkLeafUid(uid: string): string
  sidebarSessions(): any[]
}
export function createSessionIndex(state: {catalog: {sessions: any[]}; live: {live: Set<string>}; sidebar: any}, pending: () => any[]): SessionIndexService
