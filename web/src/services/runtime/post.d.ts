export interface PostEnvironment {
  fetch: typeof fetch
  appUrl(path: string): string
  buildId: string
  pageId: string
  browserAuditEvent(event: string, data?: any, content?: any, fields?: any): void
  markStaleBuild(build: string): void
}
export function createPostService(environment: PostEnvironment): {
  post(path: string, body: any, options?: {timeoutMs?: number}): Promise<any>
}
