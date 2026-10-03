import { defineStore } from 'pinia'
import { ref, shallowRef } from 'vue'

export const useSessionCatalogStore = defineStore('runtime-session-catalog', () => {
  const sessions = shallowRef<any[]>([])
  const sig = ref<string | null>(null)
  return { sessions, sig }
})
export const useSelectionStore = defineStore('runtime-selection', () => {
  const sel = ref<string | null>(null)
  const agent = ref<string | null>(null)
  const syncGap = ref(350)
  const lastSync = ref(0)
  return { sel, agent, syncGap, lastSync }
})
export const useSessionLiveStore = defineStore('runtime-session-live', () => {
  const live = shallowRef(new Set<string>())
  const liveTmux = shallowRef(new Set<string>())
  const liveWorking = shallowRef(new Set<string>())
  const liveStarted = shallowRef(new Map<string, number>())
  return { live, liveTmux, liveWorking, liveStarted }
})
export const useUnreadStore = defineStore('runtime-unread', () => {
  const unread = shallowRef(new Map<string, any>())
  const cursors = shallowRef(new Map<string, any>())
  return { unread, cursors }
})
export const useSidebarPreferencesStore = defineStore('runtime-sidebar-preferences', () => {
  const view = ref('tree')
  const nest = ref(false)
  const nestClosed = shallowRef(new Set<string>())
  const off = shallowRef(new Set<string>())
  const closed = shallowRef(new Set<string>())
  const activeOnly = ref(false)
  const compactTurns = ref(true)
  const starBusy = shallowRef(new Set<string>())
  const picking = ref(false)
  const nestAttach = ref('')
  const nestAttachUids = shallowRef<string[]>([])
  return { view, nest, nestClosed, off, closed, activeOnly, compactTurns, starBusy, picking, nestAttach, nestAttachUids }
})
export const useSessionSearchStore = defineStore('runtime-session-search', () => {
  const term = ref('')
  const opts = shallowRef({case: false, word: false, regex: false, mode: 'all'})
  const cur = ref(-1)
  const autoOpen = ref(0)
  const markCapped = ref(false)
  const results = shallowRef<any[] | null>(null)
  const searchClosed = shallowRef(new Set<string>())
  const searchNestClosed = shallowRef(new Set<string>())
  return { term, opts, cur, autoOpen, markCapped, results, searchClosed, searchNestClosed }
})
