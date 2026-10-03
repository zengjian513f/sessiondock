import type { ConversationStore } from '../../stores/conversation'
import { operations } from './bridge'

// A scoped generation cancels lazy materialization after view replacement.
// Reopening uses the same materialized rows, including full-output controls.
export function openDisclosure(store: ConversationStore, id: string, count: () => number) {
  const state = store.disclosure(id)
  state.open = true
  if (state.materialized) { operations().publishStore(store); return }
  state.materialized = true; state.visible = 0
  const generation = store.state.generation
  const paint = () => {
    if (generation !== store.state.generation) return
    const started = performance.now()
    do {
      state.visible=Math.min(count(),state.visible+16)
      operations().publishStore(store)
    } while (state.visible < count() && performance.now() - started < 8)
    if (state.visible < count()) setTimeout(paint, 0)
    else state.visible = Infinity
  }
  if (count()) paint()
}
