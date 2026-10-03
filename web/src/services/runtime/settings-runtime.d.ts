export interface SettingsRuntimeService {
  machineTargets(...args: any[]): any
  CONSOLE_RENDERERS: any
  localConsoleRenderer(...args: any[]): any
  consolePasteFilesEnabled(...args: any[]): any
  settingsValues(...args: any[]): any
  mountSettings(...args: any[]): any
}
export function createSettingsRuntime(dependencies: {terminal: () => any; core: import('./core').RuntimeCore; sleep: () => any; bulk: () => any; appearance: () => any; capabilities: import('./core').RuntimeCapabilities; sidebarResources: () => any}): SettingsRuntimeService
