import { createApp, h, reactive } from 'vue'
import type { App } from 'vue'
import MediaError from '../../components/overlays/MediaError.vue'
import type { MediaDiagnostic, MediaView } from './media'

interface InlineMediaServices {
 lazyMediaEnabled(): boolean
 safeMediaSrc(path: string): string
 captureView(element: HTMLElement, currentGeneration: () => number): MediaView
 diagnoseMedia(path: string): Promise<MediaDiagnostic>
 forgetMediaDiagnostic(path: string): void
 reloadMediaSession(view: MediaView, button: {disabled: boolean}, notice: {textContent: string}): Promise<void>
}
interface MediaWrapper extends HTMLElement {_mediaGeneration?: number}
interface Panel {
 wrapper: MediaWrapper
 host: HTMLSpanElement
 app?: App
 retry(): void
 reload(): Promise<void>
 view: MediaView
 state: {detail: MediaDiagnostic & {pending?: boolean}; notice: string; reloadBusy: boolean}
}

// The established Markdown fragment still owns its image/link markup. This
// adapter owns only the small Vue error panel and its lifetime, never image GETs.
export function createInlineMedia(services: InlineMediaServices, contains: (element: Node) => boolean) {
 const panels = new Map<MediaWrapper, Panel>()
 let dormant = new WeakMap<MediaWrapper, Panel>()
 let started = false
 let observer: MutationObserver | undefined
 function release(panel: Panel, preserve = false) {
  if (panels.get(panel.wrapper) !== panel) return
  panels.delete(panel.wrapper)
  panel.app?.unmount(); panel.app = undefined
  panel.host.remove()
  if (preserve) dormant.set(panel.wrapper, panel)
  else dormant.delete(panel.wrapper)
 }
 function mount(panel: Panel) {
  dormant.delete(panel.wrapper)
  const {state} = panel
  panel.app = createApp({setup: () => () => h(MediaError, {detail:state.detail, notice:state.notice, reloadBusy:state.reloadBusy, onRetry:panel.retry, onReload:panel.reload})})
  panels.set(panel.wrapper, panel)
  panel.wrapper.append(panel.host)
  panel.app.mount(panel.host)
 }
 function prune() {
  for (const panel of panels.values()) {
   if (!panel.wrapper.isConnected || !contains(panel.wrapper)) release(panel, true)
   else if (!panel.wrapper.contains(panel.host)) release(panel)
  }
 }
 // A cached Markdown fragment can return with the same hidden image/link.
 // Resume its saved Vue panel without retrying the image or its diagnostic.
 function restore(node: Node) {
  if (!(node instanceof Element)) return
  const wrappers = [...node.querySelectorAll<MediaWrapper>('.media-load')]
  if (node.matches('.media-load')) wrappers.unshift(node as MediaWrapper)
  for (const wrapper of wrappers) {
   const panel = dormant.get(wrapper)
   if (panel && wrapper.isConnected && contains(wrapper)) mount(panel)
  }
 }
 async function failed(event: Event) {
  const img = event.target
  if (!(img instanceof HTMLImageElement) || img.dataset.vueMedia === 'true'
      || !services.lazyMediaEnabled() || img.dataset.mediaLazy !== 'true') return
  const path = img.dataset.mediaPath
  const wrapper = img.closest<MediaWrapper>('.media-load')
  if (!/^\/api\/media\/[0-9a-f]{32}$/.test(path || '') || !wrapper?.isConnected || !contains(wrapper)) return
  const src = services.safeMediaSrc(path!)
  if (img.src !== new URL(src, location.href).href) return
  wrapper._mediaGeneration = (wrapper._mediaGeneration || 0) + 1
  const view = services.captureView(wrapper, () => wrapper._mediaGeneration || 0)
  const link = img.closest<HTMLAnchorElement>('.media-link')!
  link.hidden = true
  const previous = panels.get(wrapper)
  if (previous) release(previous)
  const host = document.createElement('span')
  const state = reactive<Panel['state']>({detail:{status:0, message:'', pending:true}, notice:'图片加载失败；正在读取错误说明…', reloadBusy:false})
  let panel: Panel
  const current = () => panels.get(wrapper) === panel && view.current()
  const retry = () => {
   if (!current()) return
   wrapper._mediaGeneration = (wrapper._mediaGeneration || 0) + 1
   services.forgetMediaDiagnostic(path!)
   release(panel)
   link.hidden = false
   img.src = src
  }
  const reload = async () => {
   await services.reloadMediaSession(view, {
    get disabled() {return state.reloadBusy}, set disabled(value) {state.reloadBusy = value},
   }, {get textContent() {return state.notice}, set textContent(value) {state.notice = value}})
  }
  panel = {wrapper, host, view, state, retry, reload}
  dormant.delete(wrapper)
  mount(panel)
  const detail = await services.diagnoseMedia(path!)
  if (!current()) return
  state.detail = detail
  state.notice = `图片不可用${detail.status ? `（HTTP ${detail.status}）` : ''}：${detail.message}`
 }
 function loaded(event: Event) {
  const img = event.target
  if (services.lazyMediaEnabled() && img instanceof HTMLImageElement && img.dataset.vueMedia !== 'true'
      && img.dataset.mediaLazy === 'true' && img.naturalWidth > 0) services.forgetMediaDiagnostic(img.dataset.mediaPath!)
 }
 function start() {
  if (started) return
  started = true
  document.addEventListener('error', failed, true)
  document.addEventListener('load', loaded, true)
  observer = new MutationObserver(records => {
   prune()
   for (const record of records) for (const node of record.addedNodes) restore(node)
  })
  observer.observe(document.documentElement, {childList:true, subtree:true})
 }
 function dispose() {
  if (!started) return
  started = false
  document.removeEventListener('error', failed, true)
  document.removeEventListener('load', loaded, true)
  observer?.disconnect(); observer = undefined
  for (const panel of panels.values()) release(panel)
  dormant = new WeakMap()
 }
 return {start, dispose}
}
