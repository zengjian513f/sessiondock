export interface SidebarResourcesService {
  cells: any
  refresh(...args: any[]): any
  toggle(...args: any[]): any
  start(...args: any[]): any
}
export function createSidebarResources(core: any,terminal: any,sessionUi: any): SidebarResourcesService
