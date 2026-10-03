import type { Pinia } from 'pinia'
import type { useCacheSettingsStore } from '../../stores/runtime/cache'
export interface CacheService {
  cache: Map<string, any>
  settings: ReturnType<typeof useCacheSettingsStore>
  queuedAfterTimestamp(uid: string): string | null
  cacheGet(key: string): any
  cacheEntryUid(key: string, entry: any): string
  cacheEntryPinned(key: string, entry: any): boolean
  trimCache(): void
  cachePut(key: string, entry: any): void
  setLimit(value: number): void
}
export function createMessageCache(pinia: Pinia, preferences: {get(key: string, fallback: any): any; set(key: string, value: any): void},
  selection: {sel: string | null; agent: string | null}, live: {liveTmux: Set<string>}, terminalRows: () => any[]): CacheService
