import type { WorkspaceState } from './workspace'
import type { NetworkService } from './network.js'
import type { SessionSyncService } from './session-sync.js'
export function createSyncTicker(network: NetworkService, selection: WorkspaceState['selection'],
  live: WorkspaceState['live'], sync: SessionSyncService): {tickSync(): void; start(): () => void}
