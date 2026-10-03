import { defineStore } from 'pinia'
import { runtimePinia } from './runtime/pinia'
import { reactive } from 'vue'
import type { SearchNode, SearchOptions } from '../domain/search/types'
export const useSearchStore = defineStore('ui-search', () => {const state = reactive({
 query: '', term: '', opts: {case: false, word: false, regex: false, mode: 'all'} as SearchOptions,
 status: '加载中…', error: false, sequence: undefined as number | undefined,
 progress: {active: false, done: 0, total: null as number | null, totalKnown: false, nodes: [] as SearchNode[]},
 summary: {active: false, full: false, query: '', count: 0},
 navigation: {text: '…', capped: false, title: undefined as string | undefined},
 revision: 0,
})
return {state}
})
export const state = useSearchStore(runtimePinia).state
// Temporary narrow adapter for status writers owned by list/load/action services.
// Vue alone renders #stat; this exposes no DOM mutation API beyond those writers.
export const status = {
 get textContent() {return state.status}, set textContent(value: string) {state.status = value},
 classList: {add(name: string) {if (name === 'err') state.error = true}, remove(name: string) {if (name === 'err') state.error = false}},
 dataset: {get seq() {return state.sequence}, set seq(value: number | undefined) {state.sequence = value}},
}
