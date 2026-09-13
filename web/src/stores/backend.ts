import { defineStore } from 'pinia'
import { ref, shallowRef } from 'vue'
import { fetchHealth } from '../api/health'
import type { HealthResponse } from '../api/health'

export type ConnectionStatus = 'idle' | 'checking' | 'online' | 'offline'

export const useBackendStore = defineStore('backend', () => {
  const status = ref<ConnectionStatus>('idle')
  const health = shallowRef<HealthResponse | null>(null)
  const error = ref<string | null>(null)

  async function check() {
    if (status.value === 'checking') return
    status.value = 'checking'
    health.value = null
    error.value = null
    try {
      health.value = await fetchHealth()
      status.value = 'online'
    } catch (cause) {
      status.value = 'offline'
      error.value = cause instanceof Error ? cause.message : '无法连接 Rust 后端'
    }
  }

  return { status, health, error, check }
})
