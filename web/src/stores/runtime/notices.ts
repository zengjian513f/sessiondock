import { defineStore } from 'pinia'
import { ref } from 'vue'
export const useRuntimeNoticesStore = defineStore('runtime-notices', () => {
  const stale = ref(false)
  const staleHidden = ref(false)
  const serverBuild = ref('')
  const hostname = ref('')
  const reloadBusy = ref(false)
  const reloadMessage = ref('自动同步已暂停，仍可编辑并自动保存草稿；发送前请重新加载。')
  const loginExpired = ref(false)
  return {stale, staleHidden, serverBuild, hostname, reloadBusy, reloadMessage, loginExpired}
})
