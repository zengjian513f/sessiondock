import { defineStore } from 'pinia'
import { reactive } from 'vue'
export type ItemMenuAction = 'copy-identity'|'stop'|'hide'|'detach'|'attach'|'delete'|'clone'|'group'|'pick'
export const useItemMenuStore = defineStore('runtime-item-menu', () => {
  const state = reactive({uid:'', hidden:true, left:'', top:'', deleteLabel:'删除会话',
    groupExpanded:false, reasons:{} as Partial<Record<ItemMenuAction,string>>})
  return {state}
})
