export interface ConversationRendererService {
  cancel(...args: any[]): any
  progress(...args: any[]): any
  progressDone(...args: any[]): any
  ensureConsolePlaceholder: any
  renderSeq: any
  renderSession(...args: any[]): any
  headerLayoutSignature: any
  auditHeaderLayout(...args: any[]): any
  refreshMessageTimeDividers(...args: any[]): any
  sealTurnTail(...args: any[]): any
  flushPendingTurnSeal(...args: any[]): any
  renderActivity(...args: any[]): any
  renderTerminalThreadNotice(...args: any[]): any
  renderConversationTail(...args: any[]): any
}
export function createConversationRenderer(dependencies: {core: import('./core').RuntimeCore; sessionUi: () => any; detailNotices: () => any; syntaxRuntime: () => any; composer: () => any; terminal: () => any; questionRuntime: () => any; pendingStage: () => any; presentation: ReturnType<typeof import('../../stores/runtime/presentation').useRuntimePresentationStore>}): ConversationRendererService
