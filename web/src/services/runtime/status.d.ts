export interface StatusService {
  sessionFrozen(...args: any[]): any
  sessionInputAttention(...args: any[]): any
  inputAttentionLabel: any
  paintItemStatus(...args: any[]): any
  sessionTurn(...args: any[]): any
  turnLabel: any
  paintTurn(...args: any[]): any
  initializeHeaderStatus(meta:any,pending:any):any
  paintHeaderTurn(...args: any[]): any
  paintLive(...args: any[]): any
  syncActiveOnlyList(...args: any[]): any
  selectSessionScope(...args: any[]): any
  sidebarScopeKeydown(...args: any[]): any
  renderSessionCounts(...args: any[]): any
  showSessionCount(...args: any[]): any
  agentRunning: any
  start(...args: any[]): any
}
export function createStatus(dependencies: {terminal: () => any; dom: () => any; composer: () => any; core: import('./core').RuntimeCore; sidebarView: () => any; status: () => any; sessionUi: () => any; conversationRenderer: () => any; sidebarGestures: () => any; bulk: () => any; filters: () => any; capabilities: import('./core').RuntimeCapabilities; presentation: ReturnType<typeof import('../../stores/runtime/presentation').useRuntimePresentationStore>}): StatusService
