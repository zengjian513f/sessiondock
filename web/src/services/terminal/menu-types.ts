import type { CompatibilityRecord } from './bridge'
export interface TerminalMenuState {
  menuHidden: boolean; searchHidden: boolean; pasteDisabled: boolean
  status: string; left: string; top: string
}
export interface TerminalMenuElements {
  menu: HTMLDivElement; search: HTMLFormElement; menuButton: HTMLButtonElement; query: HTMLInputElement
}
export interface TerminalMenuContext {
  T: CompatibilityRecord
  copyTermSelection(term: CompatibilityRecord): void
  showConsoleToast(message: string): void
  gestures: {pinching: boolean; isTouchEvent(event: Event): boolean}
}
export interface TerminalMenuActions {
  paste(): Promise<void>
  showSearch(): void
  toggleMenu(): void
  searchKeydown(event: KeyboardEvent): void
  menuKeydown(event: KeyboardEvent): void
  closeSearch(): void
  input(): void
  previous(): void
  next(): void
}
