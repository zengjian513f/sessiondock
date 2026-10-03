export interface MarkdownRuntimeService {
  CLIP: any
  AUTO_OPEN_MAX: any
  MARK_MAX: any
  clipText: any
  md(...args: any[]): any
  RE_LIST: any
  RE_HEAD: any
  RE_QUOTE: any
  RE_HR: any
  isSep: any
  cells: any
  isTable: any
  blocks(...args: any[]): any
  RE_MD_IMAGE: any
  RE_CODE_SPAN: any
  RE_REFERENCE: any
  isWindowsDrivePath: any
  fileParentDirectory(...args: any[]): any
  trimReference(...args: any[]): any
  referenceLink(...args: any[]): any
  inline(...args: any[]): any
  start(...args: any[]): any
}
export function createMarkdownRuntime(dependencies: {dom: () => any; core: import('./core').RuntimeCore; mediaRuntime: () => any; sidebarGestures: () => any; capabilities: import('./core').RuntimeCapabilities}): MarkdownRuntimeService
