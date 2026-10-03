import { defineStore } from 'pinia'
import { runtimePinia } from './runtime/pinia'
import { shallowReactive } from 'vue'
import type { SidebarView, PickBar, Chip, ResourceCell } from '../domain/sidebar/types'
export const useSidebarStore = defineStore('ui-sidebar', () => {
 const state = shallowReactive({
    view: null as SidebarView | null,
    pick: null as PickBar | null,
    sources: [] as Chip[], nodes: [] as Chip[],
    resources: new Map<string, ResourceCell[]>(),
  })
 return {state}
})
export function createSidebarStore() {return useSidebarStore(runtimePinia).state}
export type SidebarStore = ReturnType<typeof createSidebarStore>
