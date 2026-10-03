import type { useOverlaysStore } from '../../stores/overlays'
export interface SleepDependencies {
 store: {get(key: string, fallback: number): unknown; set(key: string, value: number): void}
 storagePrefix: string
 network: {pause(reason: string): void; resume(): void}
 settings: {update(values: {sleep: number}): void}
}
export function createSleep(state: ReturnType<typeof useOverlaysStore>, deps: SleepDependencies) {
 const choices = new Set([0, 5, 15, 30, 60, 120, 240])
 const normalized = (value: unknown) => value !== null && value !== '' && choices.has(Number(value)) ? Number(value) : 60
 let minutes = normalized(deps.store.get('sleepMinutes', 60))
 let lastActivity = Date.now(), timer = 0, sleeping = false
 let dialog: HTMLDialogElement
 function schedule() {
  clearTimeout(timer)
  if (!sleeping && minutes) timer = window.setTimeout(check, Math.max(1, lastActivity + minutes * 60000 - Date.now()))
 }
 function sleep() {
  sleeping = true; state.sleeping = true
  clearTimeout(timer)
  deps.network.pause('idle')
  dialog.showModal()
 }
 function resume() {
  sleeping = false; state.sleeping = false; lastActivity = Date.now()
  dialog.close(); schedule(); deps.network.resume()
 }
 function check() {
  if (sleeping) return
  if (minutes && Date.now() - lastActivity >= minutes * 60000) sleep()
  else schedule()
 }
 function activity(event: Event) {
  if (!event.isTrusted) return
  check()
  if (sleeping) {
   if (!dialog.contains(event.target as Node)) {
    if (event.cancelable) event.preventDefault()
    event.stopImmediatePropagation()
   }
   return
  }
  lastActivity = Date.now()
 }
 const activityEvents = ['pointerdown', 'pointermove', 'keydown', 'wheel', 'touchstart', 'input']
 const checkEvents = ['focus', 'pageshow', 'visibilitychange']
 function configure(value: unknown, persist = false) {
  if (persist) {deps.store.set('sleepMinutes', normalized(value)); lastActivity = Date.now()}
  minutes = normalized(value); deps.settings.update({sleep: minutes}); check()
 }
 function storage(event: StorageEvent) {
  if (event.key === deps.storagePrefix + 'sleepMinutes' || event.key === null) configure(deps.store.get('sleepMinutes', 60))
 }
 function start() {
  activityEvents.forEach(type => addEventListener(type, activity, {capture:true, passive:false}))
  checkEvents.forEach(type => addEventListener(type, check, true))
  addEventListener('storage', storage); schedule()
 }
 function dispose() {
  clearTimeout(timer); activityEvents.forEach(type => removeEventListener(type, activity, true))
  checkEvents.forEach(type => removeEventListener(type, check, true)); removeEventListener('storage', storage)
 }
 return Object.freeze({configure, resume, start, dispose, attach(element: HTMLDialogElement) {dialog = element}, get lastActivity() {return lastActivity}, get minutes() {return minutes}, get sleeping() {return sleeping}})
}
export type SleepController = ReturnType<typeof createSleep>
