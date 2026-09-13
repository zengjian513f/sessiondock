export interface HealthResponse {
  status: 'ok'
  service: 'sessiondock'
  version: string
  api_version: 1
  stage: 'scaffold' | 'read_only'
}

// TypeScript types do not validate network data. Keep the runtime boundary explicit.
export function parseHealth(value: unknown): HealthResponse {
  if (typeof value !== 'object' || value === null) {
    throw new Error('后端健康检查响应格式不正确')
  }
  const row = value as Record<string, unknown>
  if (
    row.status !== 'ok' || row.service !== 'sessiondock'
    || typeof row.version !== 'string' || row.version.length === 0
    || row.api_version !== 1 || (row.stage !== 'scaffold' && row.stage !== 'read_only')
  ) {
    throw new Error('后端健康检查协议不匹配')
  }
  return {
    status: row.status,
    service: row.service,
    version: row.version,
    api_version: row.api_version,
    stage: row.stage,
  }
}

export async function fetchHealth(): Promise<HealthResponse> {
  const response = await fetch('/api/health', {
    cache: 'no-store',
    signal: AbortSignal.timeout(5000),
    headers: { Accept: 'application/json' },
  })
  if (!response.ok) {
    throw new Error(`后端健康检查失败：HTTP ${response.status}`)
  }
  return parseHealth(await response.json())
}
