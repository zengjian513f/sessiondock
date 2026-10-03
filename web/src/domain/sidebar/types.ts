export interface Agent { id: string; title: string; type: string; updated: string; created: string; snippet?: string; hits?: number; hits_capped?: boolean; [key: string]: any }
export interface Session { uid: string; sid: string; source: string; title: string; updated: string; cwd?: string; node_id?: string; node_name?: string; agent_items?: Agent[]; group?: string; pending?: boolean; starred?: boolean; [key: string]: any }
export interface NestRow { s: Session; agent: Agent | null; depth: number; kids?: number; closed?: boolean }
export interface Status { classes: Record<string, boolean>; title: string; text: string | number; frozen?: boolean; marker?: string }
export interface RowView { key: string; row: NestRow; classes: string; title: string; titleHtml: string; meta: string; snippet: string; snippetHtml: string; icon: string; iconColor: string; starBusy: boolean; pickable: boolean; picked: boolean; group: string; status: Status; directory?: {path: string; leaf: string; node: string; color: string; label?: string; pathColor?: string} }
export interface GroupView { key: string; label: string; path: string; node: string; nodeColor: string; count: number; closed: boolean; rows: RowView[]; pickUids: string[]; picked: boolean; indeterminate: boolean }
export interface Chip { key: string; label: string; count: number; on: boolean; icon?: string; color?: string; abbr?: string; reason: string; title: string; offline?: boolean; issue?: boolean }
export interface PickBar { hidden: boolean; attaching: boolean; label: string; groupHidden: boolean; groupDisabled: boolean; allDisabled: boolean; allLabel: string; cancelDisabled: boolean; deleteLabel: string; deleteDisabled: boolean; attachHidden: boolean; attachLabel: string; attachDisabled: boolean; stopLabel: string; stopDisabled: boolean; stopBusy: boolean; stopTitle: string; detailsHidden: boolean; summary: string; errors: string }
export interface SidebarView { groups: GroupView[]; empty: string; searching: boolean; picking: boolean; attaching: boolean; createGroup: boolean; groupMode: boolean; groupBusy: boolean; groupEditing: boolean }
export interface ResourceCell { field: string; icon: string; label: string; value: string }
export interface SidebarActions {
  rowClick(row: NestRow, event: MouseEvent): void; star(uid: string): void; nestFold(uid: string): void; groupFold(key: string, event: MouseEvent): void; groupPick(uids: string[], node: HTMLElement): void;
  createGroup(input: HTMLInputElement): void; removeGroup(name: string): void; editGroup(on: boolean): void; exitSearch(): void;
  pickAll(): void; pickStop(): void; pickGroup(event: MouseEvent): void; pickAttach(): void; pickDelete(): void; pickCancel(): void;
  sourceClick(key: string): void; nodeClick(key: string): void;
  resourceOpen(session: Session): void; resourceCells(session: Session, agent: boolean): ResourceCell[];
  rowMounted(node: HTMLElement, row: NestRow): void; rowUnmounted(node: HTMLElement): void;
}
export interface SidebarGestures {
  mousedown(event: MouseEvent): void; pointerdown(event: PointerEvent): void; pointermove(event: PointerEvent): void; pointerup(event: PointerEvent): void; pointercancel(event: PointerEvent): void; pointerleave(event: PointerEvent): void; contextmenu(event: MouseEvent): void; clickCapture(event: MouseEvent): void; selectstart(event: Event): void;
}
