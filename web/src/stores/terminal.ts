import { defineStore } from 'pinia'
import { runtimePinia } from './runtime/pinia'
import { shallowReactive } from 'vue'
import type { TerminalController } from '../services/terminal/bridge'

export interface TerminalUiState {
  visible: boolean
  collapsed: boolean
  height: string
  mobileTop: string
  replay: boolean
  outputNotice: string
  shiftSelect: boolean
  ctrlArmed: boolean
  altArmed: boolean
  timeline: {
    seek: string; time: string; valueText: string; title: string; status: string
    bounds: string; ticks: string[]; playing: boolean; playLabel: string; speed: string; live: boolean
  }
}
// Only rendered presentation is reactive. Hosts, sockets, timers, buffers and
// immutable instance/lease bindings remain in the controller closure.
export const useTerminalStore = defineStore('ui-terminal', () => { const state = shallowReactive<TerminalUiState>({
  visible: false, collapsed: false, height: '', mobileTop: '', replay: false,
  outputNotice: '', shiftSelect: false, ctrlArmed: false, altArmed: false,
  timeline: {seek: '1000', time: '00:00 / 00:00', valueText: '', title: '',
    status: '会话已结束 · 只读回放', bounds: '', ticks: [], playing: false,
    playLabel: '播放', speed: '1', live: false},
})
return {state}
})
export const terminalState = useTerminalStore(runtimePinia).state
let controller: TerminalController
export function bindTerminalController(value: TerminalController) { controller = value }
export function terminalOperations(): TerminalController { return controller }
