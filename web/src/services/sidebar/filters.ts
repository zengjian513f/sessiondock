export interface FilterActions {selectOnly(button: HTMLElement): boolean; holdMs: number; slop: number}
export function createFilterGestures(actions: FilterActions) {
  let press: {button: HTMLElement; host: HTMLElement; pointerId: number; x: number; y: number; timer: ReturnType<typeof setTimeout>} | null = null
  let suppress: HTMLElement | null = null
  const button = (event: Event) => (event.target as HTMLElement).closest<HTMLElement>('button[data-node], button[data-source]')
  const cancel = () => {if (press) clearTimeout(press.timer); press = null}
  const move = (event: PointerEvent) => {
    if (!press || press.pointerId !== event.pointerId) return
    if (!press.host.contains(event.target as Node) || Math.abs(event.clientX - press.x) > actions.slop || Math.abs(event.clientY - press.y) > actions.slop) cancel()
  }
  // Motion/release can land in the host's gap between buttons. Keep the same
  // host-scoped hold cancellation while Vue binds the actual chip controls.
  document.addEventListener('pointermove', move)
  document.addEventListener('pointerup', event => {if (press?.pointerId === event.pointerId) cancel()})
  document.addEventListener('pointercancel', event => {if (press?.pointerId === event.pointerId) cancel()})
  return {
    contextmenu(event: MouseEvent) {const control = button(event); if (!control) return; event.preventDefault(); actions.selectOnly(control)},
    pointerdown(event: PointerEvent) {
      if (event.pointerType === 'mouse' || event.button !== 0) return
      const control = button(event); if (!control) return; cancel()
      const current = {button: control, host: control.parentElement!, pointerId: event.pointerId, x: event.clientX, y: event.clientY, timer: 0 as unknown as ReturnType<typeof setTimeout>}
      current.timer = setTimeout(() => {
        if (press !== current) return
        press = null; if (!actions.selectOnly(control)) return
        suppress = control; navigator.vibrate?.(12)
        setTimeout(() => {if (suppress === control) suppress = null}, 800)
      }, actions.holdMs)
      press = current
    },
    pointermove: move,
    pointerup: cancel, pointercancel: cancel,
    pointerleave(event: PointerEvent) {
      const host = (event.currentTarget as HTMLElement).parentElement
      if (host?.contains(event.relatedTarget as Node | null)) return
      cancel()
    },
    clickCapture(event: MouseEvent) {
      const control = button(event); if (!control || control !== suppress) return
      suppress = null; event.preventDefault(); event.stopImmediatePropagation()
    },
    dblclick(event: MouseEvent) {if (button(event)?.dataset.node) {event.preventDefault(); event.stopImmediatePropagation()}},
  }
}
export type FilterGestures = ReturnType<typeof createFilterGestures>
