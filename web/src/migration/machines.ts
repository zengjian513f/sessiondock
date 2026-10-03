import { createApp, h, shallowRef } from 'vue'
import SettingsMachines from '../components/SettingsMachines.vue'
import { createMachinesStore } from '../stores/machines'
import type { MachinesBridge } from '../services/machines'
import type { MachinesStore } from '../stores/machines'

let controller: MachinesStore | undefined
export function mount(bridge: MachinesBridge): void {
  if (controller) return
  controller = createMachinesStore(bridge)
  const state = shallowRef(controller.state)
  controller.subscribe(snapshot => { state.value = snapshot })
  const owner = controller
  createApp({setup: () => () => h(SettingsMachines, {state: state.value, controller: owner})}).mount('#settings-machines')
  controller.refresh()
}
export function renderMachineSettings(): void { controller?.refresh() }
export function renderClientMatrix(): void { controller?.renderMatrix() }
export function setMachineNote(text: string, error = false): void { controller?.note(text, error) }
export function open(): void { controller?.open() }
