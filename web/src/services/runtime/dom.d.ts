export interface DomService {
  el: any
  esc: any
  icon: any
  liveStatusTitle: any
  uiIcon: any
}
export function createDom(dependencies: {core: import('./core').RuntimeCore; status: () => any; capabilities: import('./core').RuntimeCapabilities}): DomService
