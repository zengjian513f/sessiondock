import { defineStore } from 'pinia'
import { ref, shallowRef } from 'vue'
export interface ShellEnvironmentItem {
  key: string
  name: string
  prefix: string
  data: {configured?: boolean; stale?: boolean; started_at?: string | number; changed?: string[]}
  restarting?: boolean
}
export const useShellEnvironmentStore = defineStore('runtime-shell-environment', () => {
  const items = shallowRef<ShellEnvironmentItem[]>([])
  const revision = ref(0)
  return { items, revision }
})
