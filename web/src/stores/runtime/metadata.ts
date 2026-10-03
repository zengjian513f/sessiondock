import { defineStore } from 'pinia'
import { shallowRef } from 'vue'
export const useMetadataOperationsStore = defineStore('runtime-metadata-operations', () => {
  const timelinePinBusy = shallowRef(new Set<string>())
  return {timelinePinBusy}
})
