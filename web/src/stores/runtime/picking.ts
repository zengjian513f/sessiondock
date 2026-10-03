import { defineStore } from 'pinia'
import { ref, shallowRef } from 'vue'
export const usePickOperationsStore = defineStore('runtime-pick-operations', () => {
  const picked = shallowRef(new Set<string>())
  const deleteBusy = ref(false)
  const stopBusy = ref(false)
  const stopProgress = shallowRef<any>(null)
  return {picked,deleteBusy,stopBusy,stopProgress}
})
