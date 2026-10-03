export interface SidebarGesturesService {
  readonly menuState: ReturnType<typeof import('../../stores/runtime/item-menu').useItemMenuStore>['state']
  setCloneReason(reason: string): void
  setGroupExpanded(expanded: boolean): void
  itemMenuAction(action: import('../../stores/runtime/item-menu').ItemMenuAction, button: HTMLButtonElement): Promise<void>
  resetItemClick(...args: any[]): any
  LONG_PRESS_MS: any
  LONG_PRESS_SLOP: any
  menuUid: any
  longPress: any
  suppressItemClick: any
  openItemMenu(...args: any[]): any
  closeItemMenu(...args: any[]): any
  cancelLongPress(...args: any[]): any
  menuTarget: any
  sidebarRenderDeferred: any
  sidebarRenderFlushTimer: any
  sidebarTextPointer: any
  sidebarTextSelectionActive(...args: any[]): any
  sidebarTextSelectionProtected: any
  scheduleDeferredSidebarRender(...args: any[]): any
  flushDeferredSidebarRender(...args: any[]): any
  sidebarGesturePointerdown1(...args: any[]): any
  sidebarGestureClick2(...args: any[]): any
  sidebarGestureContextmenu1(...args: any[]): any
  sidebarGesturePointerdown2(...args: any[]): any
  sidebarGesturePointermove1(...args: any[]): any
  sidebarGestureClick3(...args: any[]): any
  sidebarVueGestures(...args: any[]): any
  start(...args: any[]): any
}
export function createSidebarGestures(dependencies: {core: import('./core').RuntimeCore; sessionUi: () => any; terminal: () => any; sidebarView: () => any; status: () => any; bulk: () => any; build: () => any; metadata: () => any; capabilities: import('./core').RuntimeCapabilities}): SidebarGesturesService
