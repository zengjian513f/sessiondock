import { defineStore } from 'pinia'
import { ref } from 'vue'

export const useNetworkStore = defineStore('runtime-network', () => {
  const stopped = ref('')
  const sleeping = ref(false)
  return { stopped, sleeping }
})
