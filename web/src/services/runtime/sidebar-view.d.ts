export interface SidebarViewService {
  showSearchMatches(rows:any[]):any
  applySourceFilterChange(...args: any[]): any
  selectOnlySource(...args: any[]): any
  selectOnlyNodeFilter(...args: any[]): any
  renderChips(...args: any[]): any
  selectOnlyFilter(...args: any[]): any
  spawnKey: any
  nestSpecParent(...args: any[]): any
  nestParentOf(...args: any[]): any
  nestEdges(...args: any[]): any
  nestTree(...args: any[]): any
  nestDescendantUids(...args: any[]): any
  applySessionNest(...args: any[]): any
  setNestAttach(...args: any[]): any
  setSessionNest(...args: any[]): any
  pickNestParent(...args: any[]): any
  nestStamp(...args: any[]): any
  sidebarGroupFolds: any
  sidebarNestFolds: any
  sidebarGroupClosed: any
  sidebarNestClosed: any
  sidebarSearchText(...args: any[]): any
  sidebarMainMatches(...args: any[]): any
  sidebarAgentItems(...args: any[]): any
  sidebarMatchCount(...args: any[]): any
  sidebarRowSnippet(...args: any[]): any
  nestSize(...args: any[]): any
  expandRows(...args: any[]): any
  rowKey: any
  groupBy(...args: any[]): any
  pendingMeta: any
  rustPendingRow: any
  forkMeta: any
  itemMeta: any
  patchSide(...args: any[]): any
  agentMeta: any
  paintAgentStatus(...args: any[]): any
  paintSidebarSelection(...args: any[]): any
  sidebarVueMounted: any
  ensureSidebarVue(...args: any[]): any
  sidebarGroupingContext(...args: any[]): any
  sidebarStatus(...args: any[]): any
  sidebarAgentRunning(...args: any[]): any
  pendingSidebarLive: any
  sidebarPendingRunning(...args: any[]): any
  sidebarPresentationState(...args: any[]): any
  sidebarPresentationHelpers(...args: any[]): any
  sidebarRowView(...args: any[]): any
  sidebarGroupView(...args: any[]): any
  refreshSidebarRows(...args: any[]): any
  stampSidebarGroups(...args: any[]): any
  renderSide(...args: any[]): any
  revealSessionInSidebar(...args: any[]): any
  renderView(...args: any[]): any
  sidebarNestContext(...args: any[]): any
  patchNestFold(...args: any[]): any
  toggleNestFold(...args: any[]): any
  start(...args: any[]): any
}
export function createSidebarView(dependencies: {core: import('./core').RuntimeCore; status: () => any; bulk: () => any; terminal: () => any; sidebarGestures: () => any; metadata: () => any; timeline: () => any; composer: () => any; filters: () => any; dom: () => any; capabilities: import('./core').RuntimeCapabilities; resources: () => any; sidebarResources: () => any}): SidebarViewService
