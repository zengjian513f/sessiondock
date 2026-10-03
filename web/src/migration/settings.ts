import { reactive, shallowRef } from 'vue'
import type { SettingsBridge, SettingsState, SettingsValues } from '../legacy/settings-bridge'
import { createMachinesStore } from '../stores/machines'
import type { MachinesBridge } from '../services/machines'
import type { MachinesStore, MachinesState } from '../stores/machines'

export const state = reactive<SettingsState>({scale: 100, font: 'ubuntu', theme: 'system', sleep: 60,
  cache: 256, stopConcurrency: 6, pasteFiles: false,
  pwa: {text: '安装到桌面', disabled: false, title: ''}})
export const tab = shallowRef('appearance')
export const machines = shallowRef<MachinesStore>()
export const machineState = shallowRef<MachinesState>()
let preferences: SettingsBridge
let readTab: () => string
let saveTab: (value: string) => void
// Stable bridge: the genuine dialog exists before PWA initialization; page-sleep
// supplies preferences and takes over the existing install service afterwards.
export const bridge: SettingsBridge = {
  read: () => preferences.read(),
  scale: (value, persist) => preferences.scale(value, persist),
  scaleIndicator: (value, active) => preferences.scaleIndicator(value, active),
  font: value => preferences.font(value), theme: value => preferences.theme(value),
  sleep: value => preferences.sleep(value), cache: value => preferences.cache(value),
  stopConcurrency: value => preferences.stopConcurrency(value),
  pasteFiles: value => preferences.pasteFiles(value),
  pwa: {install: () => preferences.pwa.install(), takeOver: render => preferences.pwa.takeOver(render)},
}
export function mount(value: SettingsBridge, tabs: {read(): string; save(value: string): void}): void {
  preferences = value; readTab = tabs.read; saveTab = tabs.save
  Object.assign(state, value.read())
  state.pwa = value.pwa.takeOver(value => { state.pwa = value })
}
export function mountMachines(value: MachinesBridge): void {
  if (machines.value) return
  const controller = createMachinesStore(value)
  machineState.value = controller.state
  controller.subscribe(value => { machineState.value = value })
  machines.value = controller
  controller.refresh()
}
export function update(values: Partial<SettingsValues>): void { Object.assign(state, values) }
export function selectTab(name: string): void {
  tab.value = ['appearance', 'features', 'machines'].includes(name) ? name : 'appearance'
  saveTab(tab.value)
  if (tab.value === 'machines') machines.value?.open()
}
export function open(): void {
  preferences.scale(preferences.read().scale, false)
  update(preferences.read())
  machines.value?.note('')
  selectTab(readTab())
  document.querySelector<HTMLDialogElement>('#settings-dialog')!.showModal()
}
export function renderMachineSettings(): void { machines.value?.refresh() }
export function renderClientMatrix(): void { machines.value?.renderMatrix() }
export function setMachineNote(text: string, error = false): void { machines.value?.note(text, error) }
export function openMachines(): void { machines.value?.open() }
