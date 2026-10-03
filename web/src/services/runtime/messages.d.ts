export interface ReadOptions {
  start?: number
  head?: string
  anchor?: string
  agent?: string | null
  appendOnly?: boolean
  windowed?: boolean
  signal?: AbortSignal
  onActivity?: () => void
  onProgress?: (done: number, total: number, detail?: {compressed: boolean; estimated: boolean}) => void
}
export interface ReadEnvironment {
  fetch: typeof fetch
  appUrl(path: string): string
  capabilities: {config: {backend?: unknown}}
  pageId: string
  buildId: string
  browserAuditEvent(event: string, data?: any, content?: any, fields?: any): void
}
export function createMessageReader(environment: ReadEnvironment): {
  fetchMessages(uid: string, options?: ReadOptions): Promise<{data: any; bytes: number; networkBytes: number}>
}
