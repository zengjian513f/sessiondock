import { describe, expect, it, vi } from 'vitest'
import { fetchHealth, parseHealth } from './health'

const valid = { status: 'ok', service: 'sessiondock', version: '0.1.0', api_version: 1, stage: 'scaffold' }

describe('health protocol', () => {
  it('accepts the scaffold response', () => {
    expect(parseHealth(valid)).toEqual(valid)
    expect(parseHealth({ ...valid, stage: 'read_only' }).stage).toBe('read_only')
  })

  it('rejects malformed and incompatible responses', () => {
    for (const input of [null, {}, { ...valid, service: 'another-service' }, { ...valid, api_version: 2 }, { ...valid, stage: 'unknown' }]) {
      expect(() => parseHealth(input)).toThrow()
    }
  })

  it('checks the HTTP status before accepting a response', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('unavailable', { status: 503 })))
    await expect(fetchHealth()).rejects.toThrow('HTTP 503')
  })
})
