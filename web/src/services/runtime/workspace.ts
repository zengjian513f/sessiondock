import type { Pinia } from 'pinia'
import { useSessionCatalogStore, useSelectionStore, useSessionLiveStore, useUnreadStore, useSidebarPreferencesStore, useSessionSearchStore } from '../../stores/runtime/workspace'

export function createWorkspaceState(pinia: Pinia,
  preferences: {get<T>(key: string, fallback: T): T}, capabilities: {allows(name: string): boolean}) {
  const catalog = useSessionCatalogStore(pinia)
  const selection = useSelectionStore(pinia)
  const live = useSessionLiveStore(pinia)
  const unread = useUnreadStore(pinia)
  const sidebar = useSidebarPreferencesStore(pinia)
  const search = useSessionSearchStore(pinia)
  sidebar.view = preferences.get('view', 'tree')
  sidebar.nest = preferences.get('nest', false)
  sidebar.nestClosed = new Set(preferences.get('nestClosed', []))
  sidebar.off = new Set(preferences.get('off', []))
  sidebar.closed = new Set(preferences.get('closed', []))
  sidebar.activeOnly = capabilities.allows('live') && preferences.get('activeOnly', false)
  sidebar.compactTurns = preferences.get('compactTurns', true)
  search.opts = Object.assign({case: false, word: false, regex: false, mode: 'all'}, preferences.get('opts', {}))
  unread.unread = new Map(preferences.get('unread', []))
  return { catalog, selection, live, unread, sidebar, search }
}
export type WorkspaceState = ReturnType<typeof createWorkspaceState>
