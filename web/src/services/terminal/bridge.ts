// Opaque existing session/renderer records stay in nonreactive controllers.
// This interface names every cross-owner dependency; it never looks up globals.
export interface CompatibilityRecord { [field: string]: any }
export interface TerminalBridge {
  fetch: typeof fetch
  pageId: string
  gestures: {pinching: boolean; isTouchEvent(event: Event): boolean}
  vendors: {
    Terminal: any
    FitAddon: any
    Unicode11Addon: any
    WebglAddon: any
    ensureAssets(grid: boolean): Promise<void>
  }
  post(url: string, body: CompatibilityRecord, options?: {timeoutMs?: number}): Promise<CompatibilityRecord>
  environment: {
    APP_BASE: URL
    HUB_MODE: boolean
    MOBILE: MediaQueryList
    Nodes: CompatibilityRecord
    SessionDockCapabilities: {config: CompatibilityRecord; allows(name: string): boolean}
    SessionDockNetwork: {paused: boolean}
    store: {get(key: string, fallback?: any): any; set(key: string, value: any): void}
    SOURCES: Record<string, {name: string; icon: string; color: string}>
    appUrl(path: string): string
    applyNodeState(data: CompatibilityRecord, kind: string): void
    nodeOf(uid: string | null): string
    newNodeId(): string
    sessionTerminalEnabled(uid: string | null): boolean
    sessiondockCli(source: string): CompatibilityRecord | undefined
  }
  sessions: {
    state: import('../runtime/workspace').WorkspaceState
    ConsoleUI: {errors: Map<string, string>; busy: Set<string>}
    cache: Map<string, CompatibilityRecord>
    forkAncestors(session: CompatibilityRecord): CompatibilityRecord[]
    forkLeafUid(uid: string): string
    loadSessions(force?: boolean): Promise<void>
    openSession(uid: string, agent?: string | null, options?: CompatibilityRecord): Promise<void>
    paintLive(): void
    pendingTmuxSessions(): CompatibilityRecord[]
    pendingUid(name: string): string
    pollLive(force?: boolean): Promise<void>
    sidebarSessions(): CompatibilityRecord[]
    trimCache(): void
    viewKey(uid: string): string
    deleteSessions(uids: string[]): Promise<void>
  }
  composer: {
    uid: string | null
    drafts: Map<string, CompatibilityRecord>
    composerDraft(uid?: string | null, create?: boolean): CompatibilityRecord
    consolePasteFiles(view: CompatibilityRecord, name: string, event: ClipboardEvent): void
    conversationSendEnabled(): boolean
    deleteComposerDraftStorage(uid: string): void
    followServerDraft(uid: string, revision?: number | null): Promise<void>
    hydrateComposerDraft(uid: string, retry?: boolean): Promise<void>
    migrateComposerDraft(from: string, to: string): void
    pollComposerInput(): Promise<void>
    recoverComposerDrafts(): Promise<void>
    recoverServerComposerDrafts(): Promise<void>
    renderComposer(): void
    renderComposerItems(): void
    sendToSession(text: string | null, keys?: string[] | null, uid?: string | null, media?: CompatibilityRecord[], options?: CompatibilityRecord): Promise<CompatibilityRecord>
    syncComposerDraftBindings(): void
    syncComposerUnloadProtection(): void
  }
  presentation: {
    appAlert(message: string): Promise<void>
    appConfirm(message: string): Promise<boolean>
    auditDetailRendered(reason: string, detail?: CompatibilityRecord): void
    browserAuditEvent(event: string, data?: CompatibilityRecord, payload?: any, context?: CompatibilityRecord): void
    ensureConsolePlaceholder(): void
    layoutHeader(): void
    renderChips(): void
    renderConversationTail(activity: CompatibilityRecord | null, uid: string): void
    renderMachineSettings(): void
    renderPendingSessionAction(info: CompatibilityRecord, button?: HTMLButtonElement | null): void
    renderSide(): void
    renderTakeoverBtn(): void
    showConsoleToast(message: string): void
    showMobileList(): void
    setPendingStage(text: string): void
    showDetailState(kind: string, text: string): void
    showNewSessionStage(info: CompatibilityRecord): void
    showSessionCount(count: number): void
    showSessionStopNotice(uid: string, message: string): void
  }
}

export interface TerminalController {
  state: CompatibilityRecord
  TERM_PAGE_ID: string
  termRows(): number
  termTheme(): any
  configuredTermFont(): any
  termFont(): any
  termFontSize(): any
  terminalFontGridRatio(family: any, size: any): any
  prepareTerminalFont(): any
  hexToRgb(hex: any): any
  hslLightness(r: any, g: any, b: any): any
  reflectedLightRgb(r: any, g: any, b: any, background?: any): any
  indexedTerminalRgb(n: any): any
  adaptRgbForLight(r: any, g: any, b: any, background: any): any
  adaptColonRgb(token: any): any
  adaptSgrBody(body: any): any
  stripOscColorSets(s: any): any
  stripOscColorReports(s: any): any
  lightTerminalAnsi(s: any): any
  terminalColorChunk(view: any, s: any): any
  refreshTerminalPreferences(redraw?: any): any
  terminalListUncertain(uid: any): any
  mergeUnavailableTermRows(rows: any, previous: any, errors: any): any
  loadTermList(): any
  fetchTermList(): any
  sessionTermMeta(uid: any): any
  termBindingServes(boundUid: any, uid: any): any
  linkedTermSession(uid: any, options?: any): any
  takenOver(uid: any): any
  termSendLease(name: any): any
  termRowBinding(name: any, uid: any): any
  termInputBody(name: any, body: any): any
  adoptLinkedTermSession(fromUid: any, linked: any, reason: any): any
  rebindSelectedTermSession(): any
  toggleLinkedTermSession(uid: any): any
  takeover(uid: any, btn: any): any
  workerStatusMessage(info: any): any
  pendingPhase(row: any): any
  pendingStateLabel(s: any): any
  pendingStageMessage(info: any): any
  pendingRecordMissing(info: any): any
  notePendingInput(name: any): any
  pendingSessionRow(name: any): any
  pendingShellRunning(row: any): any
  pendingTitle(info: any): any
  refreshPendingStage(name: any): any
  notePendingEnded(name: any): any
  pendingSelectionGone(name: any): any
  stopPendingSession(info: any, button: any, pending?: (value: boolean) => void): any
  deletePendingSession(info: any, button: any, pending?: (value: boolean) => void): any
  discardPendingSession(info: any): any
  sessionIsPtyOnly(uid?: any): any
  sessionTerminalFirst(uid?: any): any
  openPendingSession(info: any): any
  discardAbandonedNewSession(info: any): any
  resolveNewSession(info: any): any
  auditTermPane(action: any, extra?: any): any
  currentTermViewObject(): any
  syncTermAliases(view?: any): any
  activateTermView(view: any): any
  requestTermFocus(view: any, source?: any): any
  focusTermIfRequested(view: any): any
  legacyCopyText(text: any, term: any): any
  copyTermSelection(term: any): any
  decodeOsc52Clipboard(payload: any): any
  handleOsc52Clipboard(view: any, payload: any): any
  rememberTermSelection(view: any): any
  restoreTermSelection(view: any): any
  termSelectionMouseDown(event: any): any
  shouldUseTermWebgl(uid?: any): any
  consoleRendererFor(row: any): any
  consoleRendererIsGrid(name: any): any
  ensureTerm(name: any): any
  writeTermOutput(view: any, chunk: any): any
  termSyncFrameOpen(s: any): any
  flushTermSyncHold(view: any): any
  terminalViewportHasCodexSideThread(term: any): any
  codexSideThreadVisible(uid?: any): any
  setCodexSideThreadState(view: any, active: any): any
  scheduleCodexSideThreadScan(view: any): any
  writeParsedTermOutput(view: any, chunk: any): any
  positionTermViewport(view: any): any
  dropTermSyncHold(view: any): any
  termPaneRenderable(view?: any): any
  repaintTermView(view: any): any
  performTermFit(view: any, forceSync?: any): any
  refreshTerminalScale(settled?: any): any
  fitTerm(immediate?: any, forceSync?: any): any
  settleActivatedTermView(view: any): any
  currentTermView(name?: any): any
  rememberTermLayout(name?: any): any
  rememberTermOpen(name: any, open: any): any
  restoreTermPane(uid: any, agent?: any): any
  loadTerminalRenderer(name: any): any
  openTermPane(name: any, autoFocus?: any, requestedMode?: any, auto?: any, directClaim?: any): any
  toggleTermPane(name: any): any
  revealConversationForPrompt(uid: any, prompt: any): any
  closeTermPane(preserveView?: any): any
  layoutTermPane(): any
  claimTermOwnership(name: any, uid?: any, binding?: any, auto?: any, direct?: any): any
  describeTermTaker(label?: any, ip?: any): any
  handleTermRevoked(view: any, ip?: any, by?: any): any
  renderTermOutputNotice(view: any): any
  recordHostExit(view: any, uid: any, event: any): any
  startShellRecordingReplay(view: any, uid: any): any
  sessionRecordingReplayable(uid: any): any
  replayTimeElapsed(ms: any): any
  renderTimeline(view?: any): any
  timelineSend(view: any, message: any): any
  timelineSeekTo(view: any, unixMs: any): any
  timelinePointerDown(): any
  timelineInput(event: Event): any
  timelineChange(event: Event): any
  timelinePointerUp(): any
  timelinePointerCancel(): any
  timelinePlay(): any
  timelineSpeed(event: Event): any
  timelineLive(): any
  attachRecordingReplay(view: any, row: any, uid: any): any
  attachTerm(name: any, auto?: any, directClaim?: any): any
  attachOwnedTerm(view: any, allowRefresh?: any, auto?: any, directClaim?: any): any
  wheelBy(deltaY: any): any
  abortWheel(): any
  setScrollPos(n: any): any
  cancelTermReconnect(view?: any): any
  cancelTermConnectTimeout(view?: any): any
  armTermConnectTimeout(view: any, ws: any, uid: any, connectionId: any): any
  dropTermSocket(view?: any): any
  cancelTermHeartbeat(view: any): any
  startTermHeartbeat(view: any, ws: any, uid: any, connectionId: any): any
  scheduleTermReconnect(view?: any): any
  reconnectTerm(view?: any): any
  suspendTerm(): any
  deactivateTermView(): any
  disposeTermView(name: any): any
  setTermShiftSelection(on: any): any
  setTermAlt(on: any): any
  applyTermAlt(data: any): any
  setTermCtrl(on: any): any
  applyTermCtrl(data: any): any
  shortcutClick(e: MouseEvent): any
  startTermDrag(e: PointerEvent): any
  finishTermDrag(e: PointerEvent): any
  backgroundTerm(): any
  foregroundTerm(force?: any): any
  pollRustTermList(): any
  start(): any
}
