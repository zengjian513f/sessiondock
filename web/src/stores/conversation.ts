import { shallowReactive, reactive, markRaw } from 'vue'
import { planMessages, planTurn, isGroupableTool, messageTimeRange, MESSAGE_TIME_GAP_MS, MESSAGE_TIME_CADENCE_MS } from '../domain/conversation/planning'
import { messageIndex } from '../domain/conversation/index'
import type { ConversationMessage, ConversationPlan } from '../services/conversation/bridge'

export interface DisclosureState {
  open: boolean; materialized: boolean; visible: number; userOpened: boolean
  full: boolean; args: boolean; wrap: boolean; view: string; hashit: boolean
  selected: (number | null)[]; text: string; submitting: number | null; details: boolean
}
export function createConversationStore() {
  const ids = new WeakMap<object, string>(); let nextId = 0
  const state = shallowReactive({
    plans: [] as ConversationPlan[], generation: 0, revision: 0, sealPending: false,
    activity: null as any, question: null as any, questionActions: null as any,
    shadow: '', sideThread: false, queued: [] as any[], returned: false,
    uid: '', agent: null as string | null, failure: null as any,
    gapError: '', gapLabel: '', gapDisabled: false, gapReloadBusy: false,
  })
  const disclosures = new Map<string, DisclosureState>()
  function messageKey(m: ConversationMessage): string {
    if (m.call_id) return `${m.role}:${m.call_id}`
    if (!ids.has(m)) ids.set(m, `message:${++nextId}`)
    return ids.get(m)!
  }
  function key(p: ConversationPlan) {
    if (p.key) return p.key
    if (p.gap) return 'history-gap'
    if (p.turn) return `turn:${p.turn.key}:${messageKey(p.turn.items[0] || {})}`
    if (p.g) return `group:${messageKey(p.g[0]!)}`
    return messageKey(p.m!)
  }
  function disclosure(id: string, initial = false) {
    let saved = disclosures.get(id)
    if (!saved) {
      saved = reactive({ open: initial, materialized: initial, visible: initial ? Infinity : 0,
        userOpened: false, full: false, args: false, wrap: false, view: 'unified', hashit: false,
        selected: [], text: '', submitting: null, details: false })
      disclosures.set(id, saved)
    }
    return saved
  }
  function identify(plans: ConversationPlan[]) {
    return plans.map(p => ({...p, key: key(p)}))
  }
  function replace(plans: ConversationPlan[], reset = false) {
    if (reset) { disclosures.clear(); state.generation++; state.sealPending = false }
    state.plans = identify(plans)
  }
  function append(messages: ConversationMessage[], openTail = false) {
    const current = [...state.plans]
    let start = current.length
    while (start > 0 && (current[start - 1]?.g || isGroupableTool(current[start - 1]?.m))) start--
    let lead = 0
    while (lead < messages.length && isGroupableTool(messages[lead])) lead++
    if (start < current.length) {
      const trailing = current.slice(start)
      // Flatten the paired visual units back to native records so a result in
      // the next packet attaches once to its existing call.
      const combined = trailing.flatMap(p => p.g || [p.m!]).flatMap(m => m.result ? [m, m.result] : [m])
      const onlyTools = lead === messages.length
      const userOpened = trailing.some(p => disclosure(key(p), !!p.open).userOpened)
      const wasOpen = trailing.some(p => p.g && disclosure(key(p), !!p.open).open)
      const keepOpen = onlyTools && (openTail || wasOpen || userOpened)
      const planned = identify(planMessages([...combined, ...messages.slice(0, lead)], {openTail: keepOpen}))
      for (const p of planned) if (p.g) {
        const saved = disclosure(key(p), keepOpen)
        saved.userOpened ||= userOpened
        saved.open = keepOpen || saved.userOpened
      }
      current.splice(start, current.length - start, ...planned)
    }
    current.push(...identify(planMessages(messages.slice(start < state.plans.length ? lead : 0), {openTail})))
    state.plans = current
  }
  function sealTools() {
    let i = state.plans.length - 1
    while (i >= 0 && (state.plans[i]?.g || isGroupableTool(state.plans[i]?.m))) {
      const p = state.plans[i--]!
      if (p.g) { const saved = disclosure(key(p), !!p.open); if (!saved.userOpened) saved.open = false }
    }
  }
  function seal(entry: any, defer: boolean, sticking: boolean, live: boolean) {
    const messages: ConversationMessage[] = entry?.msgs || []
    const activity = entry?.activity
    const index=messageIndex(messages)
    if (['working', 'waiting'].includes(activity?.state) && !index.tailHasFinal) return false
    const start=index.turnStart
    if (start < 0) { if (activity?.state !== 'working') sealTools(); return false }
    const complete = (activity?.state && !['working','waiting'].includes(activity.state)) || (!activity && !live)
    const plan = planTurn(messages.slice(start), {complete, openTail: activity?.state === 'working'})
    if (!plan.some((p: ConversationPlan) => p.turn)) { if (activity?.state !== 'working') sealTools(); return false }
    const first = messageKey(messages[start]!)
    const rendered = state.plans.findIndex(p => p.m && messageKey(p.m) === first)
    if (rendered < 0 || state.plans[rendered]?.sealedTurnHead) return false
    if (defer && !sticking) { state.sealPending = true; return false }
    state.plans = [...state.plans.slice(0, rendered), ...identify(plan)]
    if (state.plans[rendered]) state.plans[rendered]!.sealedTurnHead = true
    state.sealPending = false
    return true
  }
  function timed(plans: ConversationPlan[]) {
    let previousEnd: number | null = null, lastShownAt: number | null = null
    return plans.flatMap(p => {
      if (p.m?.silent || (p.m?.role === 'question' && !p.m.live && p.m.call_id === state.shadow)) return [p]
      if (p.gap || p.m?.role === 'event') {
        previousEnd = null; if (p.gap) lastShownAt = null
        return [p]
      }
      const range = messageTimeRange(p.turn?.items || p.g || (p.m ? [p.m] : []))
      if (!range) { previousEnd = null; lastShownAt = null; return [p] }
      if (lastShownAt === null) lastShownAt = range.start
      const show = (previousEnd !== null && range.start - previousEnd > MESSAGE_TIME_GAP_MS)
        || range.start - lastShownAt! > MESSAGE_TIME_CADENCE_MS
      previousEnd = Math.max(range.start, range.end)
      if (show) { lastShownAt = range.start; return [{key:`time:${key(p)}`, time:range.start} as any, p] }
      return [p]
    })
  }
  // Raw messages belong to the cache service. Vue observes plans and scoped UI
  // state; transport packets do not turn the shared cache into deep proxies.
  return markRaw({state, disclosure, key, messageKey, identify, replace, append, seal, sealTools, timed})
}
export type ConversationStore = ReturnType<typeof createConversationStore>
