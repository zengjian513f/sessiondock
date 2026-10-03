import { mobile, operations } from '../../stores/shell'
const VISUAL_KEYBOARD_INSET_MIN = 120
let visualLayoutWidth = 0, visualLayoutHeight = 0, visualKeyboardWasOpen = false, viewportFrame = 0
export function browserPinchZoomed() {
  return Math.abs((window.visualViewport?.scale || 1) - 1) > 0.01
}
export function visualKeyboardOpen() {
  if (!mobile.matches) return false
  if (browserPinchZoomed()) return visualKeyboardWasOpen
  const viewport = window.visualViewport
  const width = Math.round(viewport?.width || window.innerWidth)
  const height = Math.max(1, Math.round(viewport?.height || window.innerHeight))
  if (width !== visualLayoutWidth) {
    visualLayoutWidth = width; visualLayoutHeight = height
    return visualKeyboardWasOpen = false
  }
  if (height > visualLayoutHeight) {
    visualLayoutHeight = height; return visualKeyboardWasOpen = false
  }
  return visualKeyboardWasOpen = visualLayoutHeight - height >= VISUAL_KEYBOARD_INSET_MIN
}
export function measureKeyboardClosedLayout<T>(measure: () => T): T {
  if (!visualKeyboardOpen()) return measure()
  const root = document.documentElement.style, saved = root.getPropertyValue('--visual-viewport-height')
  root.setProperty('--visual-viewport-height', `${visualLayoutHeight}px`)
  try { return measure() } finally {
    if (saved) root.setProperty('--visual-viewport-height', saved)
    else root.removeProperty('--visual-viewport-height')
  }
}
export function syncMobileViewport() {
  cancelAnimationFrame(viewportFrame)
  viewportFrame = requestAnimationFrame(() => {
    const root = document.documentElement.style
    if (!mobile.matches) {
      visualLayoutWidth = 0; visualLayoutHeight = 0; visualKeyboardWasOpen = false
      root.removeProperty('--visual-viewport-height'); root.removeProperty('--visual-viewport-top'); return
    }
    if (browserPinchZoomed()) return
    const viewport = window.visualViewport
    const height = Math.max(1, Math.round(viewport?.height || window.innerHeight))
    const top = Math.max(0, Math.round(viewport?.offsetTop || 0))
    root.setProperty('--visual-viewport-height', `${height}px`)
    root.setProperty('--visual-viewport-top', `${top}px`)
    if (operations().layoutTerminal()) {
      if (visualKeyboardOpen()) { operations().scrollTerminal(); operations().positionTerminal() }
      else operations().fitViewportTerminal()
    }
  })
}
export function initializeViewport(): () => void {
  window.visualViewport?.addEventListener('resize', syncMobileViewport)
  window.visualViewport?.addEventListener('scroll', syncMobileViewport)
  window.addEventListener('resize', syncMobileViewport)
  syncMobileViewport()
  return () => {
    cancelAnimationFrame(viewportFrame)
    window.visualViewport?.removeEventListener('resize', syncMobileViewport)
    window.visualViewport?.removeEventListener('scroll', syncMobileViewport)
    window.removeEventListener('resize', syncMobileViewport)
  }
}
