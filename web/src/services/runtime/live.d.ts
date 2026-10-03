import type { WorkspaceState } from './workspace'
import type { NetworkService } from './network.js'
export function createLiveService(environment: {capabilities: {allows(name: string): boolean}; network: NetworkService; fetch: typeof fetch; appUrl(path: string): string},
  catalog: WorkspaceState['catalog'], state: WorkspaceState['live'], nodes: {applyNodeState(data: any, context: string): void},
  terminal: {state: any; loadTermList(): Promise<any>; closeTermPane(preserveView?: boolean): void},
  presentation: {paintLive(): void; renderTakeoverBtn(): void}, trimCache: () => void): {
  refreshLive(force?: boolean): Promise<void>
  pollLive(force?: boolean): Promise<any>
  runLivePoll(force: boolean): Promise<void>
}
