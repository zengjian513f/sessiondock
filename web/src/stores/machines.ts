import { createMachinesService } from '../services/machines'
import type { MachineClient, MachineTarget, MachinesBridge } from '../services/machines'
import { compareVersions, clientCell, clientUpdateSummary } from '../domain/client-versions'

export const MACHINE_COLORS = [
  ['', '默认'], ['blue', '蓝'], ['violet', '紫'], ['amber', '琥珀'], ['teal', '青'],
  ['rose', '玫红'], ['lime', '青柠'], ['cyan', '天蓝'], ['fuchsia', '品红'],
];
interface ClientEntry {clients: MachineClient[] | null; error: string; seq: number; loading?: boolean}
export interface MachinesState {
  targets: MachineTarget[];
  clients: Map<string, ClientEntry>;
  busy: Set<string>;
  controls: Map<string, string | boolean>;
  note: string;
  error: boolean;
  palette: string | null;
  editing: boolean;
  dragging: string | null;
  rowRevision: number;
}
// The store owns state and timers, independent of Vue and DOM. The migration
// entry subscribes to immutable UI snapshots; callbacks only measure or focus.
export function createMachinesStore(bridge: MachinesBridge) {
  const service = createMachinesService(bridge);
  const state: MachinesState = {targets: [], clients: new Map(), busy: new Set(), controls: new Map(), note: '', error: false,
    palette: null, editing: false, dragging: null, rowRevision: 0};
  const listeners = new Set<(value: MachinesState) => void>();
  const polls = new Map<string, ReturnType<typeof setTimeout>>();
  let orderTimer: ReturnType<typeof setTimeout> | undefined;
  let orderVersion = 0;
  const emit = () => {
    const snapshot = {...state, targets: state.targets.map(t => ({...t})), busy: new Set(state.busy), controls: new Map(state.controls),
      clients: new Map([...state.clients].map(([id, e]) => [id, {...e, clients: e.clients?.map(c => ({...c, update: c.update && {...c.update}})) || null}]))};
    for (const listener of listeners) listener(snapshot);
  };
  const note = (text: string, error = false) => { state.note = text || ''; state.error = error; emit(); };
  const name = (client: MachineClient) => bridge.sourceNames[client.source]?.name || client.source;
  const summary = (target: MachineTarget, client: MachineClient) => clientUpdateSummary(target, client, name(client));
  const entryFor = (target: MachineTarget) => {
    let entry = state.clients.get(target.id);
    if (!entry) { entry = {clients: null, error: '', seq: 0}; state.clients.set(target.id, entry); }
    return entry;
  };
  function refresh() {
    if (!state.editing && !state.dragging && !orderTimer) {
      state.targets = bridge.readTargets(); state.palette = null; state.rowRevision++;
      state.controls.clear();
    }
    emit();
  }
  function schedule(target: MachineTarget) {
    clearTimeout(polls.get(target.id)); polls.delete(target.id);
    if (!(state.clients.get(target.id)?.clients || []).some(c => c.update?.running || c.latest_state === 'pending')) return;
    polls.set(target.id, setTimeout(() => void loadClients(target), 2000));
  }
  async function loadClients(target: MachineTarget) {
    const entry = entryFor(target), seq = ++entry.seq;
    entry.loading = true;
    if (!entry.clients) emit();
    try {
      const data = await service.clients(target);
      if (seq !== entry.seq) return;
      const before = new Map((entry.clients || []).map(c => [c.id, c]));
      entry.clients = Array.isArray(data.clients) ? data.clients : [];
      entry.error = '';
      for (const client of entry.clients || []) {
        if (before.get(client.id)?.update?.running && client.update && !client.update.running)
          note(summary(target, client), !client.update.ok);
      }
    } catch (error) {
      if (seq !== entry.seq) return;
      entry.error = message(error);
    } finally { if (seq === entry.seq) entry.loading = false; }
    emit(); schedule(target);
  }
  async function updateClient(target: MachineTarget, client: MachineClient) {
    const key = `client:${target.id}:${client.id}`;
    state.busy.add(key); note(`${target.name}：正在更新 ${name(client)}…`);
    try {
      await service.update(target, client.id);
      // Mutate the canonical entry, never a UI snapshot. Sequence invalidation
      // protects the locally marked update from an earlier outstanding GET.
      const entry = entryFor(target);
      const canonical = entry.clients?.find(c => c.id === client.id) || client;
      canonical.update = {...canonical.update, running: true};
      entry.seq++; entry.loading = false;
    } catch (error) { note(`${target.name}：${name(client)} 更新失败：${message(error)}`, true); }
    state.busy.delete(key); emit(); schedule(target);
  }
  async function postDisplay(target: MachineTarget, patch: object, key: string) {
    state.busy.add(key); note('正在保存…');
    try { return await service.display(target, patch); }
    finally { state.busy.delete(key); emit(); }
  }
  async function save(target: MachineTarget, patch: {name?: string; color?: string; enabled?: boolean}, key: string) {
    if (target.local) return;
    const before = {name: target.name, color: target.color, enabled: target.enabled !== false};
    if ((patch.name ?? target.name) === target.name && (patch.color ?? target.color) === target.color
      && (patch.enabled ?? before.enabled) === before.enabled) return;
    if (patch.name !== undefined) state.controls.set(key, patch.name);
    if (patch.enabled !== undefined) state.controls.set(key, patch.enabled);
    try {
      const data = await postDisplay(target, patch, key);
      Object.assign(target, {name: data.node.name, color: data.node.color, enabled: data.node.enabled});
      bridge.applyDisplay(target, data.node);
      // Keep the server response visible even while a name control has focus.
      const own = state.targets.find(t => t.id === target.id);
      if (own) Object.assign(own, target);
      if (data.node.enabled !== before.enabled) {
        note(data.node.enabled ? `已重新接入 ${data.node.name}，正在检查…`
          : `已停用 ${data.node.name}：不显示、不检查，视同不存在。`);
        await bridge.loadNodes(); refresh();
        await Promise.allSettled([bridge.loadSessions(), bridge.refreshLive(), bridge.loadTermList()]);
        refresh(); return;
      }
      note(`已保存 ${data.node.name}。`); bridge.renderNodes(); refresh();
    } catch (error) {
      state.controls.delete(key);
      if (!key.startsWith('name:') && !key.startsWith('enabled:')) refresh();
      note(`保存失败：${message(error)}`, true);
    }
  }
  async function chooseRenderer(target: MachineTarget, value: string) {
    if (value === target.renderer) return;
    const label = bridge.rendererOptions.find(([v]) => v === value)?.[1] || value;
    if (target.local) {
      bridge.setLocalRenderer(value); target.renderer = value;
      const own = state.targets.find(t => t.id === target.id); if (own) own.renderer = value;
      note(`本机：控制台改用 ${label}（保存在此浏览器）；重新打开控制台后生效。`); return;
    }
    state.controls.set(`renderer:${target.id}`, value);
    try {
      const data = await postDisplay(target, {renderer: value}, `renderer:${target.id}`);
      target.renderer = data.node?.renderer || value;
      bridge.applyRenderer(target, target.renderer);
      const own = state.targets.find(t => t.id === target.id); if (own) own.renderer = target.renderer;
      state.controls.delete(`renderer:${target.id}`);
      note(`${target.name}：控制台改用 ${label}；重新打开控制台后生效。`);
    } catch (error) { state.controls.delete(`renderer:${target.id}`); note(`${target.name}：切换失败：${message(error)}`, true); }
  }
  async function saveOrder(ids: string[]) {
    const version = orderVersion, current = () => version === orderVersion;
    try {
      const data = await service.order(ids); note('顺序已保存。');
      if (!current()) return;
      bridge.applyOrder(data.machines); await bridge.loadNodes();
      if (current()) refresh();
    } catch (error) {
      note(`顺序保存失败：${message(error)}`, true);
      if (!current()) return;
      await bridge.loadNodes().catch(() => {}); if (current()) refresh();
    }
  }
  function move(id: string, step: number) {
    const index = state.targets.findIndex(t => t.id === id), next = index + step;
    if (next < 0 || next >= state.targets.length) return;
    const [target] = state.targets.splice(index, 1); state.targets.splice(next, 0, target!);
    orderVersion++;
    const ids = state.targets.map(t => t.id); clearTimeout(orderTimer);
    orderTimer = setTimeout(() => { orderTimer = undefined; void saveOrder(ids); }, 400); emit();
  }
  function dragOver(id: string, other: string, after: boolean) {
    const ids = state.targets.map(t => t.id), index = ids.indexOf(id), otherIndex = ids.indexOf(other);
    const desired = otherIndex + (after ? 1 : 0);
    if (desired === index || desired === index + 1) return;
    const [target] = state.targets.splice(index, 1);
    state.targets.splice(desired > index ? desired - 1 : desired, 0, target!); emit();
  }
  function matrix(snapshot: MachinesState) {
    const machines = bridge.readTargets().filter(t => t.enabled !== false);
    const installed = (id: string) => (snapshot.clients.get(id)?.clients || []).filter(c => c.installed);
    const sources = bridge.sources.filter(source => machines.some(t => installed(t.id).some(c => c.source === source)));
    const reference: Record<string, string> = {}, published = new Set<string>();
    for (const target of machines) for (const client of installed(target.id)) {
      if (client.latest) published.add(client.source);
      for (const version of [client.latest, client.version]) if (version
        && (!reference[client.source] || compareVersions(version, reference[client.source]!) > 0)) reference[client.source] = version;
    }
    return {sources, rows: machines.map(target => {
      const entry = snapshot.clients.get(target.id);
      const unreachable = target.online === false || (entry?.error && !entry.clients);
      const status = unreachable ? '离线' : !entry || (entry.loading && !entry.clients) ? '…' : '';
      return {target, status, error: entry?.error || '', cells: sources.map(source => {
        const client = installed(target.id).find(c => c.source === source);
        return {source, client, ...(client ? clientCell(client, reference[source], published.has(source), summary(target, client)) : {})};
      })};
    })};
  }
  return {state, bridge, refresh, renderMatrix: emit, note, loadClients, updateClient, save, chooseRenderer, move, dragOver, matrix,
    subscribe(fn: (value: MachinesState) => void) { listeners.add(fn); emit(); return () => { listeners.delete(fn); }; },
    open() { refresh(); for (const target of bridge.readTargets()) if (target.enabled !== false && target.online !== false && !polls.has(target.id)) void loadClients(target); },
    palette(id: string | null) { state.palette = id; emit(); },
    editing(value: boolean) { state.editing = value; },
    draft(key: string, value: string) { state.controls.set(key, value); },
    startDrag(id: string) { state.dragging = id; emit(); },
    finishDrag(before: string[]) { state.dragging = null; emit(); const ids = state.targets.map(t => t.id);
      if (ids.join() !== before.join()) { orderVersion++; void saveOrder(ids); } },
  };
}
function message(error: unknown): string { return error instanceof Error ? error.message : String(error); }
export type MachinesStore = ReturnType<typeof createMachinesStore>;
