export interface MachineTarget {
  id: string; name: string; color: string; online?: boolean | null; local: boolean;
  enabled: boolean; renderer: string; terminal?: boolean
}
export interface ClientUpdate {
  running?: boolean; ok?: boolean; before?: string; after?: string; output?: string; code?: number | null
}
export interface MachineClient {
  id: string; source: string; installed: boolean; version?: string; detail?: string;
  latest?: string; latest_state?: string; update?: ClientUpdate
}
export interface MachineNode {
  id: string; name: string; color?: string; enabled?: boolean; renderer?: string; online?: boolean
}
export interface MachinesBridge {
  readTargets(): MachineTarget[];
  appUrl(path: string): string;
  sourceNames: Record<string, {name: string}>;
  sources: string[];
  rendererOptions: string[][];
  offlineReason(target: MachineTarget): string;
  setLocalRenderer(value: string): void;
  applyDisplay(target: MachineTarget, node: MachineNode): void;
  applyRenderer(target: MachineTarget, renderer: string): void;
  applyOrder(machines: MachineNode[]): void;
  loadNodes(): Promise<unknown>;
  loadSessions(): Promise<unknown>;
  refreshLive(): Promise<unknown>;
  loadTermList(): Promise<unknown> | null;
  renderNodes(): void;
}
export function machineApi(target: MachineTarget, path: string): string {
  return target.local ? path : `api/nodes/${target.id}/${path}`;
}
export function createMachinesService(bridge: MachinesBridge) {
  async function request(path: string, body?: unknown) {
    const response = await fetch(bridge.appUrl(path), body === undefined
      ? {cache: 'no-store'}
      : {method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify(body)});
    const data = await response.json().catch(() => ({}));
    if (body === undefined && response.status === 404 && !data.code)
      throw new Error('这台机器的 SessionDock 版本不支持查看客户端');
    if (!response.ok || data.error) throw new Error(data.error || `请求失败（HTTP ${response.status}）`);
    return data;
  }
  return {
    clients: (target: MachineTarget) => request(machineApi(target, 'api/clients')),
    update: (target: MachineTarget, id: string) => request(machineApi(target, 'api/clients/update'), {id}),
    display: (target: MachineTarget, patch: object) => request(`api/nodes/${target.id}/display`, patch),
    order: (ids: string[]) => request('api/nodes/order', {ids}),
  };
}
