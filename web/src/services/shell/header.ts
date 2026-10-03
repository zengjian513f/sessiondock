import { nextTick } from 'vue'
import { state, actionDefinitions, mobile, medium, readActionAttributes, operations } from '../../stores/shell'
import type { ActionId } from '../../stores/shell'
export const displayMode = matchMedia('(display-mode: standalone)')
export function closeMenu(restoreFocus = false) {
  if (!state.menuOpen) return
  state.menuOpen = false
  if (restoreFocus) document.getElementById('header-more-btn')?.focus()
}
let running = false, pending = false
// Each measurement follows Vue's patch in the same microtask turn, before paint.
// The original squeeze thresholds and fold/unfold ordering are unchanged.
export async function layoutHeader() {
  readActionAttributes()
  if (running) { pending = true; return }
  running = true
  try {
    do {
      pending = false
      await nextTick()
      await measure()
    } while (pending)
  } finally { running = false }
}
async function measure() {
  const header = document.querySelector<HTMLElement>('header')
  const filters = header?.querySelector<HTMLElement>('.header-filters')
  if (!header || !filters || getComputedStyle(header).display === 'none') return
  const squeezed = () => filters.scrollWidth > filters.clientWidth || header.scrollWidth > header.clientWidth
  const picker = document.getElementById('node-picker')
  const canFoldNodes = !!(picker && !picker.hidden)
  const inline = () => actionDefinitions.map(a => a.id).filter(id => {
    const a = state.actions[id]
    return !a.hidden && !a.docked && !state.folded.includes(id)
  })
  if (mobile.matches && canFoldNodes) { state.nodes = true; await nextTick() }
  if (squeezed()) {
    closeMenu()
    for (const key of ['labels', 'brand', 'nodes'] as const) {
      if (key === 'nodes' && !canFoldNodes) continue
      if (state[key]) continue
      state[key] = true; await nextTick()
      if (!squeezed()) return
    }
    const buttons = inline()
    for (let i = buttons.length - 1; i >= 0 && squeezed(); i--) {
      state.folded = [buttons[i]!, ...state.folded]; await nextTick()
    }
    return
  }
  if (state.folded.length) closeMenu()
  while (state.folded.length) {
    const id = state.folded[0]!
    state.folded = state.folded.slice(1); await nextTick()
    if (!squeezed()) continue
    state.folded = [id, ...state.folded]; await nextTick(); return
  }
  if (mobile.matches) return
  for (const key of ['nodes', 'brand', 'labels'] as const) {
    if (!state[key]) continue
    state[key] = false; await nextTick()
    if (squeezed()) { state[key] = true; await nextTick(); return }
  }
}
export function dockAction(id: ActionId, docked: boolean): boolean {
  if (state.actions[id].docked === docked) return false
  state.actions[id].docked = docked
  closeMenu(); state.folded = []
  return true
}
export function setTransfers(count: number) {
  state.actions['transfer-tasks'].hidden = count === 0
  state.actions['transfer-tasks'].count = String(count)
  void nextTick().then(layoutHeader)
}
export function syncPageReload() {
  state.actions['page-reload'].hidden = !(displayMode.matches
    || (navigator as Navigator & {standalone?: boolean}).standalone === true)
  if (state.actions['page-reload'].hidden) state.folded = state.folded.filter(id => id !== 'page-reload')
  void nextTick().then(layoutHeader)
}
export function watchHeader(): () => void {
  const observer = new ResizeObserver(() => void layoutHeader())
  const header = document.querySelector('header')!
  for (const node of [header, header.querySelector('.brand')!, ...header.querySelector('.header-filters')!.children]) observer.observe(node)
  const mutations = new MutationObserver(records => {
    let globals = false, controls = false
    for (const record of records) {
      const id = (record.target as HTMLElement).id
      if (!actionDefinitions.some(action => action.id === id)) continue
      controls = true
      if (['new-session', 'settings', 'page-reload', 'transfer-tasks'].includes(id)) globals = true
    }
    if (controls) void layoutHeader()
    if (globals) operations().layoutSessionHead()
  })
  mutations.observe(header, {subtree: true, attributes: true, attributeFilter: ['hidden', 'class', 'disabled']})
  const standalone = displayMode
  standalone.addEventListener('change', syncPageReload)
  const changed = () => void layoutHeader()
  mobile.addEventListener('change', changed); medium.addEventListener('change', changed)
  const resized = () => closeMenu()
  window.addEventListener('resize', resized)
  syncPageReload()
  return () => {
    observer.disconnect(); mutations.disconnect(); standalone.removeEventListener('change', syncPageReload)
    mobile.removeEventListener('change', changed); medium.removeEventListener('change', changed)
    window.removeEventListener('resize', resized)
  }
}
