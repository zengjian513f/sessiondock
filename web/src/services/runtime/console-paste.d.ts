export interface ConsolePasteService {
  consolePasteFiles(...args: any[]): any
  consolePasteJobs: any
  consoleAttachmentPath(...args: any[]): any
  publishConsolePaste(...args: any[]): any
  bindFileDrop(...args: any[]): any
}
export function createConsolePaste(dependencies: {settingsRuntime: () => any; terminal: () => any; composer: () => any; core: import('./core').RuntimeCore; post: () => any}): ConsolePasteService
