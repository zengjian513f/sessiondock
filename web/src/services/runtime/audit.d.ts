import type { NetworkService } from './network.js'
export interface AuditEnvironment {
  fetch: typeof fetch
  appUrl(path: string): string
  capabilities: {allows(name: string): boolean; config: {backend?: unknown}}
  network: NetworkService
  pageId: string
  buildId: string
  selected(): string | null
}
export interface AuditService {
  auditPayload(events: any[]): string
  readonly queue: any[]
  readonly sending: boolean
  readonly failureCount: number
  readonly queueLength: number
  browserAuditEvent(event: string, data?: any, content?: any, fields?: any): void
  flushBrowserAudit(): Promise<void>
  flushBrowserAuditBeacon(): void
}
export function createAuditService(environment: AuditEnvironment): AuditService
