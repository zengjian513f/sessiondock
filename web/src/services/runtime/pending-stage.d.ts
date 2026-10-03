export interface PendingStageService {
  showNewSessionStage(...args: any[]): any
  renderPendingSessionAction(...args: any[]): any
  renderQueuedSends(...args: any[]): any
  selectPendingSidebarRow(...args: any[]): any
}
export function createPendingStage(dependencies: {terminal: () => any; core: import('./core').RuntimeCore; conversationRenderer: () => any; composer: () => any; pendingStage: () => any; status: () => any; sessionUi: () => any; takeover: () => any; sidebarView: () => any; capabilities: import('./core').RuntimeCapabilities}): PendingStageService
