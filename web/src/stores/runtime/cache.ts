import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

// Raw native history entries belong to the cache controller, never a deep proxy.
export const useCacheSettingsStore = defineStore('runtime-history-cache', () => {
  const limitMb = ref(256)
  const maxBytes = computed(() => limitMb.value ? limitMb.value * 1024 * 1024 : Infinity)
  return { limitMb, maxBytes }
})
