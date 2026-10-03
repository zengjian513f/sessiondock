export interface RuntimeEnvironment {
  HUB_MODE: boolean
  STORAGE_PREFIX: string
  MOBILE: MediaQueryList
  MEDIUM: MediaQueryList
  layoutTier(): 'narrow' | 'medium' | 'wide'
  APP_BASE: URL
  DEEP_SID: string
  deepNode(): string
  appUrl(path: string): string
  BUILD_ID: string
  AUDIT_PAGE_ID: string
}
export function createEnvironment(capabilities: {namespace: string}, selectedNodeIds: () => string[], newNodeId: () => string): RuntimeEnvironment
