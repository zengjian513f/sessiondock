export interface StorageCapabilities {
  namespace: string
  stored(key: string, prefix?: string): string | null
}

// Preferences use exactly the established keys and missing/corrupt fallback.
// Draft content remains owned by the server-backed composer service.
export function createPreferences(capabilities: StorageCapabilities, prefix: string) {
  return {
    get<T>(key: string, fallback: T): T {
      try {
        const value = capabilities.stored(key, prefix)
        return value === null ? fallback : JSON.parse(value)
      } catch { return fallback }
    },
    set(key: string, value: unknown) {
      localStorage.setItem(prefix + key, JSON.stringify(value))
    },
  }
}
