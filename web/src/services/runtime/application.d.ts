export interface RuntimeApplicationService {
  overlays: typeof import('../../migration/overlays')
  settings:typeof import('../../migration/settings')
  machines:typeof import('../../migration/machines')
  shell:typeof import('../../migration/shell')
  search:typeof import('../../migration/search')
  capabilities:typeof import('../overlays/capabilities').capabilities
  cli:typeof import('../../domain/runtime/cli.js')
  format:typeof import('../../domain/runtime/format.js')
  viewKey:typeof import('../../domain/runtime/messages.js').viewKey
  GridTerm:any
  readonly shellEnvironment:ReturnType<typeof import('./shell-environment').createShellEnvironment>
  core: import('./core').RuntimeCore
  terminal: import('../terminal/bridge').TerminalController
  composer: any
  sessionUi: any
  launch: any
  metadata: any
  bulk: any
  post: any
  build: any
  appearance: any
  sleep: any
  resources: any
  filters: any
  dom: any
  timeline: any
  status: any
  catalogMeta: any
  sidebarGestures: any
  sidebarView: any
  conversationRenderer: any
  syntaxRuntime: any
  formulaRuntime: any
  questionRuntime: any
  markdownRuntime: any
  mediaRuntime: any
  settingsRuntime: any
  takeover: any
  pendingStage: any
  consolePaste: any
  sidebarResources: any
  detailNotices: any
  start(...args: any[]): any
}
export function createRuntimeApplication(): RuntimeApplicationService
