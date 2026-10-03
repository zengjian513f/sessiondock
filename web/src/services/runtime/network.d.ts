import type { Pinia } from 'pinia'
export interface NetworkService {
  fetch: typeof fetch
  pause(reason: string): void
  resume(): void
  readonly paused: boolean
  readonly reason: string
}
export function createNetworkService(pinia: Pinia, capabilities: {config: {list_delta?: boolean}}, options?: {fetch?: typeof fetch}): NetworkService
