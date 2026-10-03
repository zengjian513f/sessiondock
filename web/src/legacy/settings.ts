import { createApp, reactive } from 'vue'
import SettingsAppearance from '../components/SettingsAppearance.vue'
import SettingsFeatures from '../components/SettingsFeatures.vue'
import type { SettingsBridge, SettingsState, SettingsValues } from './settings-bridge'

let state: SettingsState

// Both pane roots remain owned by the legacy dialog/tab shell. Mount replaces
// their initial markup in place, before the deferred scripts finish startup.
export function mount(bridge: SettingsBridge): void {
  state = reactive({
    ...bridge.read(),
    pwa: bridge.pwa.takeOver(value => { state.pwa = value }),
  })
  createApp(SettingsAppearance, { state, bridge }).mount('#settings-appearance')
  createApp(SettingsFeatures, { state, bridge }).mount('#settings-features')
}

export function update(values: Partial<SettingsValues>): void {
  Object.assign(state, values)
}
