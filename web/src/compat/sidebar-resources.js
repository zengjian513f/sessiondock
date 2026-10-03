/* Optional resource column using the existing sidebar controls and icons. */
(() => {
  'use strict';
  let enabled = !!store.get('sidebarResources', false);
  const button = document.querySelector('#sidebar-resources-toggle');
  let rows = new Map(), localNode = '', sampledAt = 0, pending = false;
  const key = s => JSON.stringify([s.node_id || localNode, s.source, s.sid]);
  const cells = (session, agent) => SessionDockSidebar.Resources.cells(session, agent, rows, localNode, sampledAt);
  const paintVisible = () => SessionDockSidebar.refreshResources(enabled);
  async function refresh() {
    if (!enabled || pending || document.hidden) return;
    pending = true;
    try {
      const response = await fetch(appUrl('api/resources/summary'), {signal:AbortSignal.timeout(4500)});
      if (!response.ok) throw new Error('resource summary unavailable');
      const data = await response.json();
      if (!Array.isArray(data.sessions) || !Number.isFinite(data.sampled_at)) throw new Error('invalid summary');
      localNode = data.node_id || ''; sampledAt = data.sampled_at;
      rows = new Map(data.sessions.map(row => [key(row.session), row.metrics]));
    } catch (_) { rows = new Map(); sampledAt = 0; }
    finally { pending = false; paintVisible(); }
  }
  function toggle(value, save = true) {
    enabled = value;
    if (save) store.set('sidebarResources', enabled);
    document.body.classList.toggle('sidebar-resources', enabled);
    button.classList.toggle('on', enabled);
    button.setAttribute('aria-pressed', String(enabled));
    button.title = enabled ? '隐藏列表资源列' : '显示列表资源列：CPU、进程、内存、GPU、磁盘读写';
    setSideWidth(store.get('width', SIDE_DEFAULT) + sideResourceExtra());
    paintVisible();
    layoutSessionHead();
    requestAnimationFrame(() => { if (typeof fitTerm === 'function' && T?.term) fitTerm(); });
    if (enabled) void refresh();
  }
  button.onclick = () => toggle(!enabled);
  globalThis.SessionDockSidebarResources = {cells, refresh};
  document.addEventListener('visibilitychange', () => {if (!document.hidden) {paintVisible(); void refresh();}});
  setInterval(() => {if (!document.hidden) {paintVisible(); void refresh();}}, 5000);
  toggle(enabled, false);
})();
