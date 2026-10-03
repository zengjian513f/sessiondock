export interface CatalogMetaService {
  mergeSessionMetaEvent(...args: any[]): any
  refreshSessionMeta(...args: any[]): any
}
export function createCatalogMeta(dependencies: {core: import('./core').RuntimeCore; conversationRenderer: () => any; sessionUi: () => any}): CatalogMetaService
