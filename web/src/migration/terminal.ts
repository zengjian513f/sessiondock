import { h, render } from 'vue'
import TerminalPane from '../components/terminal/TerminalPane.vue'
import { terminalState, bindTerminalController } from '../stores/terminal'
import { createTerminalController } from '../services/terminal/controller.js'
import type { TerminalBridge, TerminalController } from '../services/terminal/bridge'
let controller: TerminalController
let container: DocumentFragment | undefined
let revision = 0
function publish() {
  if (container) render(h(TerminalPane, {revision: ++revision}), container as unknown as HTMLElement)
}
export function createController(bridge: TerminalBridge): TerminalController {
  controller = createTerminalController(bridge, terminalState, publish)
  bindTerminalController(controller)
  return controller
}
export function mount(host: HTMLElement) {
  // Keep #termpane a direct #right child for the unchanged original CSS.
  container = document.createDocumentFragment()
  publish()
  host.replaceWith(...container.childNodes)
}
export function operations(): TerminalController { return controller }
