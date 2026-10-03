import { h, render, shallowReactive } from 'vue'
import TerminalMenu from '../../components/terminal/TerminalMenu.vue'
import { createTermMenuController } from './menu-controller.js'
import type { CompatibilityRecord } from './bridge'
import type { TerminalMenuContext, TerminalMenuElements, TerminalMenuState } from './menu-types'
export function mountTerminalMenu(view: CompatibilityRecord, context: TerminalMenuContext) {
  const container = document.createDocumentFragment()
  const state = shallowReactive<TerminalMenuState>({
    menuHidden: true, searchHidden: true, pasteDisabled: false, status: '', left: '', top: '',
  })
  let revision = 0
  const connect = (elements: TerminalMenuElements) => createTermMenuController(view, elements, state, publish, context)
  function publish() { render(h(TerminalMenu, {state, revision: ++revision, connect}), container as unknown as HTMLElement) }
  publish()
  view.host.append(...container.childNodes)
  return {dispose() { render(null, container as unknown as HTMLElement) }}
}
