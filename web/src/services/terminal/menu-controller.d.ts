import type { CompatibilityRecord } from './bridge'
import type { TerminalMenuActions, TerminalMenuContext, TerminalMenuElements, TerminalMenuState } from './menu-types'
export function createTermMenuController(view: CompatibilityRecord, elements: TerminalMenuElements, state: TerminalMenuState, publish: () => void, context: TerminalMenuContext): TerminalMenuActions
