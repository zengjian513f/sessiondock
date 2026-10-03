import { shallowReactive } from 'vue'
import type { SidebarView, PickBar, Chip, ResourceCell } from '../domain/sidebar/types'
export function createSidebarStore() {
  return shallowReactive({
    view: null as SidebarView | null,
    pick: null as PickBar | null,
    sources: [] as Chip[], nodes: [] as Chip[],
    resources: new Map<string, ResourceCell[]>(),
  })
}
export type SidebarStore = ReturnType<typeof createSidebarStore>
