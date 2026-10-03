export interface TakeoverService {
  renderTakeoverBtn(...args: any[]): any
}
export function createTakeover(dependencies: {terminal: () => any; core: import('./core').RuntimeCore; dom: () => any; composer: () => any; presentation: ReturnType<typeof import('../../stores/runtime/presentation').useRuntimePresentationStore>}): TakeoverService
