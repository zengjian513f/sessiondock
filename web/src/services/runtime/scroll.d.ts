import type { Pinia } from 'pinia'
import type { useConversationScrollStore } from '../../stores/runtime/scroll'
export interface ConversationScroll {
  state: ReturnType<typeof useConversationScrollStore>
  start(): () => void
  lockStick(ms?: number): void
  atBottom(box: HTMLElement): boolean
  stickBottom(box: HTMLElement, force?: boolean): void
  mutateKeepingMessageAnchor<T>(anchor: HTMLElement, mutate: () => T): T
  jumpWithinConversation(target: HTMLElement, options?: {block?: ScrollLogicalPosition}): void
  disposeMessageObservers(box: HTMLElement | null): void
  watchBottom(box: HTMLElement): void
  settle(box: HTMLElement): void
}
export function createConversationScroll(pinia: Pinia, flushPendingTurnSeal: (box: HTMLElement) => void): ConversationScroll
