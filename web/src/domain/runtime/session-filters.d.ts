import type { WorkspaceState } from '../../services/runtime/workspace'
import type { SessionIndexService } from '../../services/runtime/session-index.js'
export function createSessionFilters(state: WorkspaceState, index: SessionIndexService,
  nodes: {nodeSelected(row: any): boolean}, groups: {matches(row: any): boolean},
  searchRows: {sidebarMainMatches(row: any): boolean; sidebarAgentItems(row: any): any[]},
  nestEdges: (rows: any[]) => {children: Map<string, any[]>}): {visible(): any[]}
