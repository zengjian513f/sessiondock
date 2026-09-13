import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { useBackendStore } from './backend'

const healthy = () => Response.json({ status: 'ok', service: 'sessiondock', version: '0.1.0', api_version: 1, stage: 'scaffold' })

describe('backend state', () => {
  beforeEach(() => { setActivePinia(createPinia()) })

  it('records a successful health check', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(healthy()))
    const backend = useBackendStore()
    await backend.check()
    expect(backend.status).toBe('online')
    expect(backend.health?.service).toBe('sessiondock')
    expect(backend.error).toBeNull()
  })

  it('reports connection failures and permits retry', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValueOnce(new Error('offline')).mockResolvedValueOnce(healthy()))
    const backend = useBackendStore()
    await backend.check()
    expect(backend.status).toBe('offline')
    expect(backend.error).toBe('offline')
    expect(backend.health).toBeNull()
    await backend.check()
    expect(backend.status).toBe('online')
    expect(backend.error).toBeNull()
  })

  it('does not start overlapping health checks', async () => {
    let resolve!: (response: Response) => void
    const request = new Promise<Response>((done) => { resolve = done })
    const fetchMock = vi.fn().mockReturnValue(request)
    vi.stubGlobal('fetch', fetchMock)
    const backend = useBackendStore()
    const first = backend.check()
    await backend.check()
    expect(fetchMock).toHaveBeenCalledTimes(1)
    resolve(healthy())
    await first
    expect(backend.status).toBe('online')
  })
})
