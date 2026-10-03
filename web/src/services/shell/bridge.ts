export interface ShellBridge {
  get<T>(key: string, fallback: T): T
  set(key: string, value: unknown): void
  openNewSession(): void
  openSettings(): void
  openTrash(): void
  openTransfers(): void
  selectView(value: string): void
  toggleNest(): void
  layoutSessionHead(): void
  fitTerminal(): void
  fitViewportTerminal(): void
  closeTerminal(): void
  selected(): boolean
  freezeOverlay(): void
  stopNotice(): void
  layoutTerminal(): boolean
  scrollTerminal(): void
  positionTerminal(): void
  cancelLongPress(): void
  closeItemMenu(): void
  resetItemClick(): void
  refreshTerminalScale(persist: boolean): void
  updateScale(value: number): void
}
