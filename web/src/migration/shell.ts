import { configure, state } from '../stores/shell'
import type { ShellBridge } from '../services/shell/bridge'
import { watchHeader } from '../services/shell/header'
import { initializeWorkspace } from '../services/shell/workspace'
import { initializeViewport } from '../services/shell/viewport'
import { initializePinch } from '../services/shell/scale'
export { layoutHeader, closeMenu as closeHeaderMenu, dockAction, setTransfers, syncPageReload, displayMode } from '../services/shell/header'
export { setSideWidth, setSideCollapsed, SIDE_DEFAULT, sideResourceExtra, showMobileDetail, showMobileList, leaveBootDetail } from '../services/shell/workspace'
export { measureKeyboardClosedLayout, visualKeyboardOpen, browserPinchZoomed, syncMobileViewport } from '../services/shell/viewport'
export { normalizedInterfaceScale, interfaceScale, applyInterfaceScale, showScaleIndicator, gestures } from '../services/shell/scale'
export function mount(bridge: ShellBridge): () => void {
  configure(bridge)
  const dispose = [watchHeader(), initializeWorkspace(), initializeViewport(), initializePinch()]
  return () => dispose.forEach(stop => stop())
}
export { configure } from '../stores/shell'
export function actionState(id: import('../stores/shell').ActionId) {
  return state.actions[id]
}

export function updateView(view: string, nest: boolean) { state.view = view; state.nest = nest }

export { takeOverResourceToggle } from '../services/shell/resources'
