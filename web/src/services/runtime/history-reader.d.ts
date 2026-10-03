export function createHistoryReader(environment: {
  fetch: typeof fetch
  appUrl(path: string): string
  pageId: string
  buildId: string
  browserAuditEvent(event: string, data?: any, content?: any, fields?: any): void
}): {
  fetchHistoryPage(uid: string, agent: string | null, cursor: string, signal: AbortSignal,
    partial: any): Promise<{data: any; bytes: number}>
}
