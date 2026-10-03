import { defineStore } from 'pinia'
import { ref, shallowRef } from 'vue'

export const useGroupsStore = defineStore('runtime-groups', () => {
  const menuHidden=ref(true),menuLeft=ref(''),menuTop=ref(''),menuItems=shallowRef<{name:string;checked:boolean}[]>([])
  const message = ref('')
  const enabled = ref(true)
  const catalog = shallowRef<string[]>([])
  const available = ref(false)
  const busy = ref(false)
  const editing = ref(false)
  return {menuHidden,menuLeft,menuTop,menuItems,message,enabled, catalog, available, busy, editing }
})
