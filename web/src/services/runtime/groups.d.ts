import type { Pinia } from 'pinia'
export interface GroupsService {
  start(): () => void
  matches(session: any): boolean
  paintPickBar(): void
  create(input: HTMLInputElement): Promise<void>
  remove(name: string): Promise<void>
  edit(on: boolean): void
  showMenu(uids: string[], anchor: HTMLElement, focus?: boolean): void
  closeMenu(): void
  escapeMenu(): boolean
  readonly busy: boolean
  readonly editing: boolean
  contains(name: string): boolean
  readonly names: string[]
  readonly available: boolean
}
export interface GroupsSessions {
  catalog: {sessions: any[]}
  sidebar: {view: string; nestAttach: string}
  search: {results: any[] | null}
  cache: Map<string, any>
  indexedSessions(): {byUid: Map<string, any>}
  loadSessions(force: boolean): Promise<any>
}
export interface GroupsSidebar {
  renderView(): void
  renderSide(): void
  renderPickBar(): void
  updateGroupMenu(items: {name: string; checked: boolean}[], busy: boolean, assign: (name: string) => void, keydown: (event: KeyboardEvent) => void): void
}
export function createGroupsService(pinia: Pinia,
  environment: {capabilities: {allows(name: string): boolean}; fetch: typeof fetch; appUrl(path: string): string},
  sessions: GroupsSessions, sidebar: GroupsSidebar, itemMenu: {readonly uid: string; close(): void; setGroupExpanded(expanded:boolean):void}): GroupsService
