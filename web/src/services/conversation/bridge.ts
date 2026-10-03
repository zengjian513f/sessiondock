// Named boundary to native CLI, cache and established Markdown services.
export type ConversationMessage = { role: string; text: any; [field: string]: any }
export type ConversationPlan = { m?: ConversationMessage; g?: ConversationMessage[]; turn?: any; gap?: any; open?: boolean; sealedTurnHead?: boolean; key?: string }
export interface ConversationBridge {
  answer: (...args: any[]) => any
  answerCliQuestionForm: (...args: any[]) => any
  cancel: (...args: any[]) => any
  claimSearchOpen: (...args: any[]) => any
  clearSyntaxPaint: (...args: any[]) => any
  diagnoseMedia: (...args: any[]) => any
  dismissQueuedSend: (...args: any[]) => any
  forgetMediaDiagnostic: (...args: any[]) => any
  hasTerm: (...args: any[]) => any
  head: (...args: any[]) => any
  jumpWithinConversation: (...args: any[]) => any
  lazyMediaEnabled: (...args: any[]) => any
  live: (...args: any[]) => any
  loadHistory: (...args: any[]) => any
  loadMedia: (...args: any[]) => any
  markInner: (...args: any[]) => any
  md: (...args: any[]) => any
  mediaContinuationEnabled: (...args: any[]) => any
  mediaMoreInfo: (...args: any[]) => any
  mutateKeepingMessageAnchor: (...args: any[]) => any
  paintSyntax: (...args: any[]) => any
  paintToolOutputDiff: (...args: any[]) => any
  pinTarget: (...args: any[]) => any
  pinTimeline: (...args: any[]) => any
  questionDraft: (...args: any[]) => any
  reloadHistory: (...args: any[]) => any
  reloadMediaOwned: (...args: any[]) => any
  renderFormulae: (...args: any[]) => any
  retryMigrationRead: (...args: any[]) => any
  retryableReadFailure: (...args: any[]) => any
  revealNativeTerminal: (...args: any[]) => any
  safeMediaSrc: (...args: any[]) => any
  saveScreenText: (...args: any[]) => any
  screenText: (...args: any[]) => any
  searchAsync: (...args: any[]) => any
  searchCanOpen: (...args: any[]) => any
  searchMessage: (...args: any[]) => any
  searchProcessAsync: (...args: any[]) => any
  sessiondockCli: (...args: any[]) => any
  settle: (...args: any[]) => any
  sticking: (...args: any[]) => any
}
export interface ConversationOperations extends ConversationBridge {
  disclosureHandle: (...args: any[]) => any
  mediaBusy: (...args: any[]) => any
  mediaError: (...args: any[]) => any
  publishStore: (...args: any[]) => any
  toolHandle: (...args: any[]) => any
}
let bridge: ConversationOperations
export function configureBridge(value: ConversationOperations) { bridge = value }
export function operations() { return bridge }
