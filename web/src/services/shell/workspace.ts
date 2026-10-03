import { nextTick } from 'vue'
import { mobile, state, operations } from '../../stores/shell'
import { layoutHeader } from './header'
import { syncMobileViewport } from './viewport'
export const SIDE_DEFAULT = 340
export const sideResourceExtra = () => document.body.classList.contains('sidebar-resources') ? 144 : 0
export function setSideWidth(px: number, save = false) {
  if (mobile.matches) { state.width = ''; return }
  const w = Math.round(Math.max(200, Math.min(px, window.innerWidth - 320)))
  state.width = w + 'px'
  document.documentElement.style.setProperty('--side-width', state.collapsed ? '0px' : w + 'px')
  if (save) operations().set('width', Math.max(200, w - sideResourceExtra()))
}
export function setSideCollapsed(collapsed: boolean, save = true) {
  state.collapsed = !!collapsed && !mobile.matches
  document.body.classList.toggle('side-collapsed', state.collapsed)
  if (save) operations().set('sideCollapsed', state.collapsed)
  const width = parseInt(state.width, 10) || operations().get('width', SIDE_DEFAULT)
  document.documentElement.style.setProperty('--side-width', state.collapsed ? '0px' : width + 'px')
  operations().layoutSessionHead()
  requestAnimationFrame(() => operations().fitTerminal())
}
export function showMobileDetail() {
  if (mobile.matches) {
    state.mobileDetail = true; document.body.classList.add('mobile-detail')
    operations().set('mobilePage', 'detail'); operations().layoutSessionHead()
  }
  operations().freezeOverlay()
}
export function showMobileList() {
  operations().closeTerminal()
  state.mobileDetail = false; document.body.classList.remove('mobile-detail')
  if (mobile.matches) operations().set('mobilePage', 'list')
  operations().layoutSessionHead(); void layoutHeader(); operations().stopNotice()
}
export function leaveBootDetail() {
  if (operations().selected() || !document.body.classList.contains('mobile-detail')) return
  state.mobileDetail = false; document.body.classList.remove('mobile-detail')
}
let pointer: number | null = null, width = 0, startWidth = 0, frame = 0
export function startDrag(event: PointerEvent) {
  if (event.button !== 0 || state.dragging) return
  state.dragging = true; pointer = event.pointerId
  width = startWidth = document.getElementById('left')!.getBoundingClientRect().width
  ;(event.currentTarget as HTMLElement).setPointerCapture?.(event.pointerId)
  document.body.classList.add('dragging'); event.preventDefault()
}
function moveDrag(event: PointerEvent) {
  if (!state.dragging || event.pointerId !== pointer) return
  width = Math.round(Math.max(200, Math.min(event.clientX, window.innerWidth - 320)))
  if (!frame) frame = requestAnimationFrame(() => {
    frame = 0; state.dragTransform = `translateX(${width - startWidth}px)`
  })
}
export function finishDrag(event: PointerEvent) {
  if (!state.dragging || event.pointerId !== pointer) return
  const commit = event.type === 'pointerup'
  state.dragging = false; pointer = null
  cancelAnimationFrame(frame); frame = 0; state.dragTransform = ''
  document.body.classList.remove('dragging')
  if (commit) setSideWidth(width, true)
}
export function initializeWorkspace(): () => void {
  const resized = () => setSideWidth(parseInt(state.width, 10) || operations().get('width', SIDE_DEFAULT))
  const changed = () => {
    if (mobile.matches) {
      const detailVisible = operations().selected() && operations().get<string>('mobilePage', 'list') === 'detail'
      if (!detailVisible) operations().closeTerminal()
      state.mobileDetail = detailVisible
    } else state.mobileDetail = false
    document.body.classList.toggle('mobile-detail', state.mobileDetail)
    operations().stopNotice(); syncMobileViewport()
    setSideWidth(operations().get('width', SIDE_DEFAULT) + sideResourceExtra())
    setSideCollapsed(operations().get('sideCollapsed', false), false)
  }
  document.addEventListener('pointermove', moveDrag)
  document.addEventListener('pointerup', finishDrag); document.addEventListener('pointercancel', finishDrag)
  window.addEventListener('resize', resized); mobile.addEventListener('change', changed)
  setSideWidth(operations().get('width', SIDE_DEFAULT) + sideResourceExtra())
  setSideCollapsed(operations().get('sideCollapsed', false), false)
  void nextTick().then(layoutHeader)
  return () => {
    cancelAnimationFrame(frame)
    document.removeEventListener('pointermove', moveDrag)
    document.removeEventListener('pointerup', finishDrag); document.removeEventListener('pointercancel', finishDrag)
    window.removeEventListener('resize', resized); mobile.removeEventListener('change', changed)
  }
}
