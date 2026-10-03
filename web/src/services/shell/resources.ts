import { state, operations } from '../../stores/shell'
// Resource polling and row painting stay with the sidebar service this batch.
// Its existing toggle is a named business action; Vue owns the header control.
let toggleAction: ((event: PointerEvent) => unknown) | null = null
export function takeOverResourceToggle(action: ((event: PointerEvent) => unknown) | null) {
  if (action) toggleAction = action
  state.resources = !!operations().get('sidebarResources', false)
}
export function toggleResources(event: MouseEvent) {
  const button = event.currentTarget as HTMLButtonElement
  const action = toggleAction || button.onclick
  toggleAction = action
  button.onclick = null
  action?.call(button, event as PointerEvent)
  state.resources = !!operations().get('sidebarResources', false)
}
