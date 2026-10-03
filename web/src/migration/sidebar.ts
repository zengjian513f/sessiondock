import { h, render } from 'vue'
import SidebarList from '../components/sidebar/SidebarList.vue'
import SidebarChips from '../components/sidebar/SidebarChips.vue'
import SidebarPickBar from '../components/sidebar/SidebarPickBar.vue'
import SidebarGroupMenu from '../components/sidebar/SidebarGroupMenu.vue'
import SidebarCounts from '../components/sidebar/SidebarCounts.vue'
import SidebarLoadFailed from '../components/sidebar/SidebarLoadFailed.vue'
import { createSidebarStore } from '../stores/sidebar'
import * as Grouping from '../domain/sidebar/grouping'
import { createFilterGestures } from '../services/sidebar/filters'
import type { FilterGestures, FilterActions } from '../services/sidebar/filters'
import * as Presentation from '../domain/sidebar/presentation'
import * as Resources from '../services/sidebar/resources'
import type { SidebarActions, SidebarGestures, SidebarView, PickBar, Chip, NestRow } from '../domain/sidebar/types'
export { Grouping, Resources, Presentation }
const state = createSidebarStore()
let actions: SidebarActions, gestures: SidebarGestures, filters: FilterGestures
let side: HTMLElement, sources: HTMLElement, nodes: HTMLElement, tools: HTMLElement
const observed = new Map<Element, NestRow>(), visible = new Set<Element>()
let resourceEnabled = false
let observer: IntersectionObserver
function drawList() {
  if (!state.view) return
  const top = side.scrollTop
  render(h(SidebarList, {view: state.view, actions, gestures, resources: state.resources}), side)
  side.classList.toggle('picking', state.view.picking)
  side.classList.toggle('attaching', state.view.attaching)
  side.classList.toggle('search-mode', state.view.searching)
  side.scrollTop = top
}
export function mount(bridge: SidebarActions, events: SidebarGestures, filterActions: FilterActions) {
  actions = bridge; gestures = events; filters = createFilterGestures(filterActions)
  side = document.querySelector('#side')!; sources = document.querySelector('#chips')!
  nodes = document.querySelector('#node-chips')!; tools = document.querySelector('#side-tools')!
  side.replaceChildren(); sources.replaceChildren(); nodes.replaceChildren(); tools.replaceChildren()
  document.querySelector('#session-scope')!.replaceChildren()
  // Existing viewport observation belongs to the resource service, not row rendering.
  observer = new IntersectionObserver(entries => {
    for (const entry of entries) {
      if (entry.isIntersecting && entry.target.isConnected) {visible.add(entry.target)}
      else visible.delete(entry.target)
    }
    refreshResources()
  }, {root: side, rootMargin: '160px 0px'})
}
export function observeRow(node: HTMLElement, row: NestRow) {
  const fresh = !observed.has(node)
  observed.set(node, row)
  // Kept only for the unreplaced sync/action services and browser fixture hooks.
  Object.assign(node, {_nestRow: row, _resourceSession: row.s, _resourceAgent: !!row.agent})
  if (fresh) observer.observe(node)
}
export function unobserveRow(node: HTMLElement) {observer.unobserve(node); observed.delete(node); visible.delete(node); state.resources.delete(node.dataset.key!)}
export function update(view: SidebarView) {
  const previous = new Map(state.view?.groups.flatMap(group => group.rows.map(row => [row.key, row] as const)) || [])
  state.view = {...view, groups: view.groups.map(group => ({...group, rows: group.rows.map(row => retainDirectory(row, previous.get(row.key)))}))}
  drawList()
}
export function updatePick(view: PickBar) {
  state.pick = view; tools.hidden = view.hidden
  render(h(SidebarPickBar, {view, actions}), tools)
}
export function updateSources(chips: Chip[]) {state.sources = chips; const left = sources.scrollLeft; render(h(SidebarChips, {chips, gestures: filters, click: actions.sourceClick}), sources); sources.scrollLeft = left}
export function updateNodes(chips: Chip[]) {state.nodes = chips; const left = nodes.scrollLeft, parentLeft = nodes.parentElement!.scrollLeft; render(h(SidebarChips, {chips, gestures: filters, nodes: true, click: actions.nodeClick}), nodes); nodes.scrollLeft = left; nodes.parentElement!.scrollLeft = parentLeft}
export function refreshResources(enabled = resourceEnabled) {
  resourceEnabled = enabled
  if (!enabled) return
  const next = new Map(state.resources)
  for (const node of visible) {
    const row = observed.get(node)
    if (!row || !node.isConnected) continue
    const key = Grouping.rowKey(row)
    next.set(key, actions.resourceCells(row.s, !!row.agent))
  }
  state.resources = next; drawList()
}
function retainDirectory(view: import('../domain/sidebar/types').RowView, previous?: import('../domain/sidebar/types').RowView) {
  return view.directory && previous?.directory && view.directory.path === previous.directory.path && view.directory.node === previous.directory.node
    ? {...view, directory: {...view.directory, label: previous.directory.label, pathColor: previous.directory.pathColor}} : view
}
export function repaintRows(project: (row: NestRow) => import('../domain/sidebar/types').RowView, uid?: string) {
  if (!state.view) return
  state.view = {...state.view, groups: state.view.groups.map(group => ({...group, rows: group.rows.map(view => !uid || view.row.s.uid === uid ? retainDirectory(project(view.row), view) : view)}))}
  drawList()
}
export function currentGroups() {return state.view?.groups || []}
export function updateGroups(groups: import('../domain/sidebar/types').GroupView[]) {
  if (!state.view) return
  const previous = new Map(state.view.groups.flatMap(group => group.rows.map(row => [row.key, row] as const)))
  state.view = {...state.view, groups: groups.map(group => ({...group, rows: group.rows.map(view => retainDirectory(view, previous.get(view.key)))}))}; drawList()
}
export function fitPaths(updates: {key: string; label?: string; color: string}[]) {
  if (!state.view) return
  const labels = new Map(updates.map(update => [update.key, update]))
  state.view = {...state.view, groups: state.view.groups.map(group => ({...group, rows: group.rows.map(view => {
    const update = labels.get(view.key)
    return update && view.directory ? {...view, directory: {...view.directory, label: update.label || view.directory.path, pathColor: update.color}} : view
  })}))}; drawList()
}
export function updateGroupMenu(items: {name: string; checked: boolean}[], busy: boolean, assign: (name: string) => void, keydown: (event: KeyboardEvent) => void) {
  render(h(SidebarGroupMenu, {items, busy, assign, keydown}), document.querySelector('#session-group-menu')!)
}
export function updateCounts(active: number, total: number, known: boolean, activeOnly: boolean, select: (active: boolean) => void, keydown: (event: KeyboardEvent) => void) {
  render(h(SidebarCounts, {active, total, known, activeOnly, select, keydown}), document.querySelector('#session-scope')!)
}
export function loadFailed(retry: () => void) {
  const top = side.scrollTop
  render(h(SidebarLoadFailed, {retry}), side)
  side.scrollTop = top
  // Resource observers only keep connected rows; the failed view owns no rows.
  state.view = null
}
