import { h, render } from 'vue'
import SearchNavigator from '../components/search/SearchNavigator.vue'
import { state, status } from '../stores/search'
export { state, status }
export * from '../services/search/runtime'
export { searchTerms, literalSource, reTerm, resetRegexSearch, regexMatches, regexCached, matchesSearch, hasTerm, hl, sidebarSnippet, markMatches, markRegexMatches, jumpMark, updateMatchNav, generation, revision, MARK_MAX, AUTO_OPEN_MAX } from '../services/search/highlight'
let navigatorHost: HTMLElement | null = null
export function mountNavigator(target: HTMLElement | null) {
 if (navigatorHost) render(null, navigatorHost)
 navigatorHost = target
 if (!target) return
 state.navigation = {text: '…', capped: false, title: undefined}
 render(h(SearchNavigator), target)
}
