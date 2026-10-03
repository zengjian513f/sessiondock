import { createApp } from 'vue'
import type { Pinia } from 'pinia'
import { useOverlaysStore } from '../stores/overlays'
import SessionResources from '../components/overlays/SessionResources.vue'
import PageSleep from '../components/overlays/PageSleep.vue'
import FileMenu from '../components/overlays/FileMenu.vue'
import { createResources } from '../services/overlays/resources'
import type { ResourceDependencies } from '../services/overlays/resources'
import { createSleep } from '../services/overlays/sleep'
import type { SleepDependencies } from '../services/overlays/sleep'
import { createFileMenu } from '../services/overlays/files'
import type { FileDependencies } from '../services/overlays/files'
import { createMedia } from '../services/overlays/media'
import type { MediaDependencies, MediaView } from '../services/overlays/media'
import { createPwaInstall } from '../services/overlays/pwa'
import { createTypography } from '../services/overlays/typography'
export { capabilities } from '../services/overlays/capabilities'
export { assets, ensureTerminalAssets } from '../services/overlays/assets'
export const pwa = createPwaInstall()
export const typography = createTypography()
let pinia: Pinia
let resources: ReturnType<typeof createResources>
let sleep: ReturnType<typeof createSleep>
let files: ReturnType<typeof createFileMenu>
let media: ReturnType<typeof createMedia>
export function initialize(value: Pinia) {pinia = value; pwa.start(); typography.start()}
export function mountResources(deps: Omit<ResourceDependencies, 'sleep'>) {
 const state = useOverlaysStore(pinia)
 resources = createResources(state, {...deps, sleep: {get lastActivity() {return sleep?.lastActivity || 0}, get sleeping() {return sleep?.sleeping || false}}})
 const host = document.createElement('div'); document.body.append(host)
 createApp(SessionResources, {controller:resources}).use(pinia).mount(host)
 resources.start()
 return {open:resources.open, setLoader:resources.setLoader}
}
export function mountSleep(deps: SleepDependencies) {
 sleep = createSleep(useOverlaysStore(pinia), deps)
 const host = document.createElement('div'); document.body.append(host)
 createApp(PageSleep, {controller:sleep}).use(pinia).mount(host)
 sleep.start(); return sleep
}
export function mountFiles(deps: FileDependencies) {
 files = createFileMenu(useOverlaysStore(pinia), deps)
 const host = document.createElement('div'); document.body.append(host)
 createApp(FileMenu, {controller:files}).use(pinia).mount(host)
 files.start()
}
export function mountMedia(deps: MediaDependencies) {media?.dispose(); media = createMedia(deps); media.startInlineMedia()}
export function captureView(element: HTMLElement, currentGeneration: () => number) {return media.captureView(element, currentGeneration)}
export const mediaState = Object.freeze({get diagnosticActive() {return media.diagnosticActive}})
export function safeMediaSrc(src: unknown) {return media.safeMediaSrc(src)}
export function lazyMediaEnabled() {return media.lazyMediaEnabled()}
export function mediaContinuationEnabled() {return media.mediaContinuationEnabled()}
export function diagnoseMedia(path: string) {return media.diagnoseMedia(path)}
export function forgetMediaDiagnostic(path: string) {media.forgetMediaDiagnostic(path)}
export function reloadMediaSession(view: MediaView, button: {disabled: boolean}, notice: {textContent: string}) {return media.reloadMediaSession(view, button, notice)}
