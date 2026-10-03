import { operations } from '../../stores/shell'
export function normalizedInterfaceScale(value: unknown) {
  const number = Number(value)
  return Number.isFinite(number) && number >= 30 && number <= 150 ? Math.max(50, Math.round(number)) : 100
}
export function interfaceScale() {
  const saved = operations().get('interfaceScale', 100), value = normalizedInterfaceScale(saved)
  if (value === 50 && Number(saved) < 50) operations().set('interfaceScale', value)
  return value
}
export function applyInterfaceScale(value: unknown = interfaceScale(), persist = false) {
  const scale = normalizedInterfaceScale(value)
  if (persist) operations().set('interfaceScale', scale)
  document.documentElement.style.setProperty('--compact-scale', String(scale / 100))
  operations().updateScale(scale); operations().refreshTerminalScale(persist)
  if (persist) window.dispatchEvent(new Event('resize'))
}
let scaleIndicatorTimer = 0, scaleIndicatorFade = 0
export function showScaleIndicator(value: unknown, active = false) {
  const indicator = document.getElementById('scale-indicator')!
  clearTimeout(scaleIndicatorTimer); clearTimeout(scaleIndicatorFade)
  indicator.textContent = `${normalizedInterfaceScale(value)}%`
  if (indicator.showPopover && !indicator.matches(':popover-open')) indicator.showPopover()
  indicator.classList.add('visible')
  if (!active) scaleIndicatorTimer = window.setTimeout(() => {
    indicator.classList.remove('visible')
    scaleIndicatorFade = window.setTimeout(() => indicator.hidePopover?.(), 180)
  }, 700)
}
interface InputOrigin extends Event {pointerType?: string; sourceCapabilities?: {firesTouchEvents: boolean}}
let pinch: {distance: number; scale: number; value: number; ids: number[]} | null = null
let pointerType = '', suppressTouchTail = false
export const gestures = {
  isTouchEvent(event: InputOrigin) {
    if (event.pointerType) return event.pointerType === 'touch'
    if (event.sourceCapabilities) return event.sourceCapabilities.firesTouchEvents
    return pointerType === 'touch'
  },
  get pinching() { return pinch !== null },
}
export function initializePinch(): () => void {
  const app = document.getElementById('app')!
  let frame = 0
  const disposers: (() => void)[] = []
  function listen<K extends keyof HTMLElementEventMap>(type: K, callback: (event: HTMLElementEventMap[K]) => void, options: AddEventListenerOptions | boolean = true) {
    app.addEventListener(type, callback, options)
    disposers.push(() => app.removeEventListener(type, callback, options))
  }
  const distance = (touches: ArrayLike<Touch>) => Math.hypot(touches[0]!.clientX - touches[1]!.clientX, touches[0]!.clientY - touches[1]!.clientY)
  const consume = (event: Event) => { if (event.cancelable) event.preventDefault(); event.stopPropagation() }
  listen('pointerdown', event => {
    pointerType = event.pointerType
    if (pointerType !== 'touch') suppressTouchTail = false
  })
  listen('keydown', () => { pointerType = 'keyboard' })
  listen('touchstart', event => {
    pointerType = 'touch'
    if (pinch) { consume(event); return }
    suppressTouchTail = false
    if (event.touches.length !== 2 || [...event.touches].some(touch =>
      !app.contains(touch.target as Node) || (touch.target as Element).closest('[data-pinch-owner], input[type="range"]'))) return
    const startDistance = distance(event.touches)
    if (startDistance < 10) return
    pinch = {distance: startDistance, scale: interfaceScale(), value: interfaceScale(), ids: [...event.touches].map(touch => touch.identifier)}
    showScaleIndicator(pinch.value, true)
    operations().cancelLongPress(); operations().closeItemMenu(); operations().resetItemClick()
    for (const target of new Set([...event.touches].map(touch => touch.target))) target.dispatchEvent(new Event('sessiondock-pinch-start', {bubbles: true}))
    consume(event)
  }, {capture: true, passive: false})
  listen('touchmove', event => {
    if (!pinch) return
    consume(event)
    const touches = pinch.ids.map(id => [...event.touches].find(touch => touch.identifier === id))
    if (event.touches.length !== 2 || touches.some(touch => !touch)) return
    pinch.value = Math.round(Math.max(50, Math.min(150, pinch.scale * distance(touches as Touch[]) / pinch.distance)))
    if (!frame) frame = requestAnimationFrame(() => {
      frame = 0
      if (pinch) { applyInterfaceScale(pinch.value); showScaleIndicator(pinch.value, true) }
    })
  }, {capture: true, passive: false})
  const finish = (event: TouchEvent) => {
    if (!pinch) return
    if (event.cancelable) event.preventDefault()
    suppressTouchTail = true
    if (event.touches.length) return
    cancelAnimationFrame(frame); frame = 0
    const value = pinch.value; pinch = null
    applyInterfaceScale(value, true); showScaleIndicator(value)
  }
  listen('touchend', finish, {capture: true, passive: false})
  listen('touchcancel', finish, {capture: true, passive: false})
  listen('pointermove', event => { if (pinch && event.pointerType === 'touch') consume(event) })
  for (const type of ['click', 'contextmenu'] as const) listen(type, event => {
    if ((pinch || suppressTouchTail) && gestures.isTouchEvent(event)) consume(event)
  })
  return () => {
    cancelAnimationFrame(frame); clearTimeout(scaleIndicatorTimer); clearTimeout(scaleIndicatorFade)
    for (const dispose of disposers) dispose()
  }
}
