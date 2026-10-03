import type { ResourceData, ResourceSession, ResourceScope } from '../../domain/overlays/resources'
import type { useOverlaysStore } from '../../stores/overlays'
export interface ResourceDependencies {fetch: typeof fetch; appUrl(path: string): string; selection(): ResourceSession | null; sleep: {readonly lastActivity: number; readonly sleeping: boolean}}
export function createResources(state: ReturnType<typeof useOverlaysStore>, deps: ResourceDependencies) {
  let probeSupported = false, observedProbeState = 'off';
  function updateProbe(data: ResourceData) {
    const nodes = data.nodes || [];
    probeSupported = nodes.some(node => node.diagnostic && node.diagnostic.state !== 'unsupported');
    observedProbeState = nodes.some(node => node.diagnostic?.state === 'active') ? 'active'
      : nodes.some(node => node.diagnostic?.state === 'starting') ? 'starting'
      : nodes.some(node => node.diagnostic?.state === 'failed') ? 'failed' : 'off';
    paintProbeStatus();
  }
  let dialog: HTMLDialogElement, currentUid = '', scope: ResourceScope = 'inclusive', generation = 0, pending = false, probePending = false;
  let loader = async (uid: string, selectedScope: ResourceScope): Promise<ResourceData> => {
    const path = `api/session/resources?${new URLSearchParams({uid, scope: selectedScope})}`;
    const response = await deps.fetch(deps.appUrl(path));
    if (!response.ok) throw new Error(response.status === 404 ? '此服务尚未提供资源统计，请更新服务端。' : `资源统计暂不可用（${response.status}）`);
    return response.json();
  };
  let requestedSession: ResourceSession | null = null;
  function selection() {
    if (requestedSession) return requestedSession;
    return deps.selection();
  }
  let lease: {uid: string; scope: ResourceScope; lease_id: string} | null = null, leaseId = '', lastSentActivity = 0, lastProbeRequest = 0, pageGone = false;
  function lastActivity() { return deps.sleep.lastActivity || 0; }
  function wantsProbe() {
    return !!dialog?.open && !pageGone && !deps.sleep.sleeping
      && Date.now() - lastActivity() < 60000 && probeSupported;
  }
  function paintProbeStatus() {
    if (!dialog) return;
    const stateValue = !wantsProbe() ? 'off' : observedProbeState === 'active' ? 'active'
      : probePending ? 'starting' : observedProbeState;
    state.probeState = stateValue;
  }
  async function reconcileProbe() {
    paintProbeStatus();
    if (probePending) return;
    const selected = selection();
    const desired = wantsProbe() && selected ? {uid:selected.uid, scope, lease_id:leaseId} : null;
    const changed = lease && (!desired || lease.uid !== desired.uid || lease.scope !== desired.scope || lease.lease_id !== desired.lease_id);
    const now = Date.now(), activity = lastActivity();
    if (!changed && (!desired || document.hidden || (lease && (activity <= lastSentActivity || now-lastProbeRequest < 10000))
        || (!lease && now-lastProbeRequest < 10000))) return;
    const target = changed ? lease : desired;
    const enabled = !changed;
    probePending = true;
    lastProbeRequest = now;
    paintProbeStatus();
    try {
      const path = 'api/session/resources/probe';
      const response = await deps.fetch(deps.appUrl(path), {
        method:'POST', headers:{'Content-Type':'application/json'}, keepalive:true, signal:AbortSignal.timeout(8000),
        body:JSON.stringify({...target, enabled, lease_seconds:Math.max(1, Math.min(60, Math.ceil((activity+60000-now)/1000)))})
      });
      if (!response.ok) throw new Error(`探测请求失败（${response.status}）`);
      const result = await response.json();
      if (enabled) { lease = target; lastSentActivity = activity; }
      const failures = (result.nodes || []).filter((node: {ok?: boolean; node_name?: string; error?: string}) => node.ok === false);
      if (dialog?.open) state.probeError = failures.map((node: {node_name?: string; error?: string}) => `${node.node_name || '关联机器'}：${node.error || '探测请求失败'}`).join('；');
      if (enabled && failures.length && failures.length === (result.nodes || []).length) observedProbeState = 'failed';
    } catch (error) {
      // A lost response can still have installed a lease. Retain its identity
      // so closing/idle can release it; the server also enforces its deadline.
      if (enabled) { lease = target; lastSentActivity = activity; }
      observedProbeState = 'failed';
      if (dialog?.open) state.probeError = (error as Error).message || '探测请求失败';
    } finally {
      if (!enabled) { lease = null; lastProbeRequest = 0; }
      probePending = false;
      paintProbeStatus();
      if (dialog?.open) void refresh();
      if (!wantsProbe() || changed) void reconcileProbe();
    }
  }

  async function refresh(force = false) {
    if (!dialog?.open || (pending && !force)) return;
    const selected = selection();
    if (!selected) { state.resourceData = undefined; state.resourceMessage = '请选择一个会话。'; state.resourceError = false; return; }
    const changed = currentUid !== selected.uid;
    currentUid = selected.uid;
    state.resourceSubtitle = selected.title || selected.sid || selected.uid;
    const ticket = ++generation;
    pending = true;
    if (changed) state.probeError = '';
    if (changed || force || (!state.resourceData && !state.resourceMessage)) { state.resourceData = undefined; state.resourceMessage = '正在读取资源采样…'; state.resourceError = false; }
    try {
      const data = await loader(currentUid, scope);
      if (ticket !== generation || !dialog.open) return;
      state.resourceData = data; state.resourceMessage = ''; state.resourceError = false;
      updateProbe(data);
      void reconcileProbe();
    } catch (error) {
      if (ticket === generation && dialog.open) { state.resourceData = undefined; state.resourceMessage = (error as Error).message || '资源统计暂不可用'; state.resourceError = true; }
    } finally { if (ticket === generation) pending = false; }
  }
  function open(session?: ResourceSession) {
    requestedSession = session?.uid ? {uid:session.uid, title:session.title, sid:session.sid} : null;

    leaseId = crypto.randomUUID();
    probeSupported = false; observedProbeState = 'off'; lastProbeRequest = 0;
    if (!dialog.open) dialog.showModal();
    refresh(true);
  }

 const intervals: number[] = []
 function attach(element: HTMLDialogElement) { dialog = element }
 function closed() { generation++; pending = false; void reconcileProbe() }
 function selectScope(value: ResourceScope) {scope = value; state.resourceScope = value; state.probeError = ''; void refresh(true)}
 function start() {
  intervals.push(window.setInterval(() => { void reconcileProbe() }, 1000))
  intervals.push(window.setInterval(() => { if (dialog?.open && !document.hidden) void refresh() }, 5000))
  addEventListener('pagehide', pagehide); addEventListener('pageshow', pageshow)
 }
 function pagehide() {pageGone = true; void reconcileProbe()}
 function pageshow() {pageGone = false}
 function dispose() {intervals.forEach(clearInterval); removeEventListener('pagehide', pagehide); removeEventListener('pageshow', pageshow)}
 return {open, refresh, closed, attach, selectScope, start, dispose, setLoader(fn: typeof loader) {loader = fn}}
}
export type ResourcesController = ReturnType<typeof createResources>
