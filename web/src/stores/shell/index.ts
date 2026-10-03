import { reactive } from 'vue'
import type { ShellBridge } from '../../services/shell/bridge'
export const actionDefinitions = [
  {id: 'new-session', label: '新建会话', icon: 'i-plus'},
  {id: 'page-reload', label: '刷新页面', icon: 'i-refresh'},
  {id: 'transfer-tasks', label: '未完成的移动与复制', icon: 'i-transfer'},
  {id: 'trash', label: '回收站', icon: 'i-trash'},
  {id: 'report-bug', label: '报告问题', icon: 'i-bug'},
  {id: 'settings', label: '设置', icon: 'i-settings'},
] as const
export type ActionId = typeof actionDefinitions[number]['id']
export const state = reactive({
  view: 'tree', nest: false, resources: false,
  collapsed: false, mobileDetail: document.body.classList.contains('mobile-detail'),
  width: '', dragging: false, dragTransform: '', menuOpen: false,
  labels: false, brand: false, nodes: false, folded: [] as ActionId[],
  actions: Object.fromEntries(actionDefinitions.map(({id}) => [id, {
    hidden: id === 'page-reload' || id === 'transfer-tasks',
    capabilityHidden: id === 'new-session', disabled: false, docked: false, count: '',
  }])) as Record<ActionId, {hidden: boolean; capabilityHidden: boolean; disabled: boolean; docked: boolean; count: string}>,
})
let bridge: ShellBridge
export const mobile = matchMedia('(max-width: 720px)')
export const medium = matchMedia('(max-width: 1199px)')
export const operations = () => bridge
export function configure(value: ShellBridge) { bridge = value }
export function invoke(id: ActionId, event: MouseEvent) {
  // term.js retains its original initialization assignment this batch. The Vue
  // control invokes the named operation once, including after a fold relocation.
  // Keep the original document delegation (including report and outside-click
  // cleanup). Retire term.js's initial direct assignment before invoking Vue's
  // action, so it cannot submit the same operation a second time.
  ;(event.currentTarget as HTMLButtonElement).onclick = null
  switch (id) {
    case 'new-session': bridge.openNewSession(); break
    case 'settings': bridge.openSettings(); break
    case 'trash': bridge.openTrash(); break
    case 'report-bug': break // The existing [data-report-bug] service delegates this click.
    case 'transfer-tasks': bridge.openTransfers(); break
    case 'page-reload': location.reload(); break
  }
}
export function readActionAttributes() {
  for (const {id} of actionDefinitions) {
    const button = document.getElementById(id) as HTMLButtonElement | null
    if (!button) continue
    const action = state.actions[id]
    action.hidden = button.hidden; action.capabilityHidden = button.classList.contains('hidden')
    action.disabled = button.disabled
  }
}
