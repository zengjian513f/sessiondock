export interface InstallState {text: string; disabled: boolean; title: string}
interface InstallPrompt extends Event {prompt(): Promise<{outcome: string}>}
export function createPwaInstall() {
 let installPrompt: InstallPrompt | null = null
 let installed = false
 let renderSettings: ((state: InstallState) => void) | null = null
 const isStandalone = () => matchMedia('(display-mode: standalone)').matches || (navigator as Navigator & {standalone?: boolean}).standalone === true
 const state = (): InstallState => {
  const active = installed || isStandalone()
  return {text:active ? '已安装' : '安装到桌面', disabled: active || !installPrompt,
   title:active ? '当前已作为独立应用运行' : installPrompt ? '安装为独立桌面应用' : '浏览器尚未提供安装能力'}
 }
 const render = () => renderSettings?.(state())
 const install = async () => {
  const prompt = installPrompt
  if (!prompt) return
  installPrompt = null; render()
  const choice = await prompt.prompt()
  if (choice.outcome === 'accepted') installed = true
  render()
 }
 function beforeinstallprompt(event: Event) {event.preventDefault(); installPrompt = event as InstallPrompt; render()}
 function appinstalled() {installPrompt = null; installed = true; render()}
 function start() {addEventListener('beforeinstallprompt', beforeinstallprompt); addEventListener('appinstalled', appinstalled); render()}
 function dispose() {removeEventListener('beforeinstallprompt', beforeinstallprompt); removeEventListener('appinstalled', appinstalled)}
 return Object.freeze({install, start, dispose, takeOver(renderPane: (state: InstallState) => void) {renderSettings = renderPane; return state()}})
}
