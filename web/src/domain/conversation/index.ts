// @ts-nocheck
import {isTurnStart,sameNativeTurn,isFinalAssistant} from "./planning"
export const messageIndexes = new WeakMap();
export function messageIndex(messages) {
  let index = messageIndexes.get(messages);
  if (!index) {
    index = {length: 0, questions: new Set(), turnStart: -1, tailHasFinal: false};
    messageIndexes.set(messages, index);
  }
  for (let i = index.length; i < messages.length; i++) {
    const message = messages[i];
    if (message.role === 'question' && message.call_id) index.questions.add(message.call_id);
    if (isTurnStart(message) && !(i > 0 && isTurnStart(messages[i - 1])
        && sameNativeTurn(messages[i - 1], message))) {
      index.turnStart = i;
      index.tailHasFinal = false;
    }
    if (isFinalAssistant(message)) index.tailHasFinal = true;
  }
  index.length = messages.length;
  return index;
}
