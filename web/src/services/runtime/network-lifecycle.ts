import type { NetworkService } from './network.js'
import type { WorkspaceState } from './workspace'
import type { SessionListService } from './session-list.js'
import type { SessionSyncService } from './session-sync.js'
import type { UiEventsService } from './ui-events.js'
export interface NetworkLifecycleDependencies {
  network: NetworkService
  selection: WorkspaceState['selection']
  list: SessionListService
  sync: SessionSyncService
  events: UiEventsService
  checkServerBuild(): Promise<void>
  pollLive(force?: boolean): Promise<any>
  flushBrowserAudit(): Promise<void>
  showLoginExpired(): void
}
export function startNetworkLifecycle(deps: NetworkLifecycleDependencies) {
  const paused = (event: Event) => {
    deps.events.closeUiEvents()
    deps.sync.closeWatch()
    deps.list.pause()
    if ((event as CustomEvent<string>).detail === 'login') deps.showLoginExpired()
  }
  const resumed = async () => {
    await deps.checkServerBuild()
    if (deps.network.paused) return
    deps.events.startUiEvents()
    void deps.list.loadSessions()
    void deps.pollLive(true)
    void deps.flushBrowserAudit()
    if (deps.selection.sel) {
      const uid = deps.selection.sel, agent = deps.selection.agent
      await deps.sync.syncSession(uid,agent)
      if (deps.selection.sel === uid && deps.selection.agent === agent) deps.sync.watchSession(uid,agent)
    }
  }
  addEventListener('sessiondock-network-paused',paused)
  addEventListener('sessiondock-network-resumed',resumed)
  return () => {
    removeEventListener('sessiondock-network-paused',paused)
    removeEventListener('sessiondock-network-resumed',resumed)
  }
}
