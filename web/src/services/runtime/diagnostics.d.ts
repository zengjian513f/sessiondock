import type { RuntimeEnvironment } from './environment.js'
import type { NetworkService } from './network.js'
import type { WorkspaceState } from './workspace'
import type { CacheService } from './cache.js'
import type { AuditService } from './audit.js'
export interface DiagnosticsService {
  longFrameEvent(entry: any): any
  observeLongFrames(): void
  browserStateSnapshot(reason?: string): {data: any; content: any}
  scheduleBrowserSnapshot(reason?: string): void
  consoleButtonState(): any
  auditConsoleButton(reason?: string): void
  scheduleConsoleButtonAudit(reason?: string): void
  auditDetailRendered(source: string, extra?: any): void
  start(): void
}
export function createDiagnostics(environment: RuntimeEnvironment & {capabilities: {allows(name: string): boolean}; network: NetworkService},
  selection: WorkspaceState['selection'], terminal: {state: any}, cache: CacheService, audit: AuditService): DiagnosticsService
