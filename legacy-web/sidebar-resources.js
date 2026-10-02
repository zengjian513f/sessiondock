/* Optional resource column using the existing sidebar controls and icons. */
(() => {
  'use strict';
  const fields = [
    ['cpu_cores', 'resource-cpu', 'CPU 核数', 'core'],
    ['process_count', 'resource-process', '进程数', 'process'],
    ['memory_pss_bytes', 'resource-memory', '内存占用（PSS）', 'bytes'],
    ['gpu_count', 'resource-gpu', 'GPU 张数', 'gpu'],
    ['proc_storage_read_bytes_per_second', 'resource-read', '磁盘读取速度', 'rate'],
    ['proc_storage_write_bytes_per_second', 'resource-write', '磁盘写入速度', 'rate'],
  ];
  let enabled = !!store.get('sidebarResources', false);
  const button = document.querySelector('#sidebar-resources-toggle');
  let rows = new Map(), localNode = '', sampledAt = 0, pending = false;
  const key = s => JSON.stringify([s.node_id || localNode, s.source, s.sid]);
  const formats = [0,1,2].map(places => new Intl.NumberFormat('zh-CN', {maximumFractionDigits:places, useGrouping:false}));
  const digits = (n, places = 1) => formats[places].format(n);
  function compact(value, unit) {
    if (unit === 'core') return value > 0 && value < .01 ? '<0.01' : digits(value, 2);
    if (unit === 'process' || unit === 'gpu') return digits(value, 0);
    if (!value) return '0';
    const units = ['B','K','M','G','T','P']; let i = 0;
    while (value >= 1024 && i < units.length - 1) {value /= 1024; i++;}
    return digits(value, value < 10 && i ? 1 : 0) + units[i] + (unit === 'rate' ? '/s' : '');
  }
  const visibleRows = new Set(), watched = new WeakSet();
  const observer = new IntersectionObserver(entries => {
    for (const entry of entries) {
      if (entry.isIntersecting && entry.target.isConnected) {
        visibleRows.add(entry.target);
        paintValues(entry.target);
      } else visibleRows.delete(entry.target);
    }
  }, {root:document.querySelector('#side'), rootMargin:'160px 0px'});
  function paint(node) {
    if (!watched.has(node)) { watched.add(node); observer.observe(node); }
    if (visibleRows.has(node)) paintValues(node);
  }
  function paintValues(node) {
    const session = node._resourceSession, meta = node.querySelector('.m');
    if (!session || !meta) return;
    if (!enabled) return;
    let panel = node.querySelector('.item-resources');
    if (!panel) {
      panel = document.createElement('button'); panel.type = 'button'; panel.className = 'item-resources';
      panel.setAttribute('aria-haspopup', 'dialog');
      panel.addEventListener('click', event => {
        event.stopPropagation();
        globalThis.SessionDockResources?.open(node._resourceSession);
      });
      panel.addEventListener('mousedown', event => event.stopPropagation());
      panel.addEventListener('keydown', event => event.stopPropagation());
      node.querySelector('.body').after(panel);
    }
    panel.setAttribute('aria-label', node._resourceAgent ? '查看所属会话资源' : '查看会话资源');
    const age = Date.now()/1000 - sampledAt;
    const metrics = age >= -5 && age <= 15 && !node._resourceAgent ? rows.get(key(session)) : null;
    const cells = fields.map(([field, icon, label, unit]) => {
      const m = metrics?.[field];
      const valid = Number.isFinite(m?.value) && m.value >= 0 && ['ok','partial'].includes(m.status);
      const value = valid ? compact(m.value, unit) : '—';
      return {field, icon, label, value};
    });
    const signature = JSON.stringify(cells);
    if (panel.dataset.resourceSignature === signature) return;
    panel.dataset.resourceSignature = signature;
    panel.innerHTML = cells.map(c => `<span class="item-resource" data-resource="${c.field}" aria-label="${esc(c.label)}：${esc(c.value)}">${uiIcon(c.icon)}<span class="item-resource-value">${esc(c.value)}</span></span>`).join('');
  }
  function paintVisible() {
    if (!enabled) return;
    for (const node of visibleRows) {
      if (node.isConnected) paintValues(node); else visibleRows.delete(node);
    }
  }
  // Unwatch discarded rows, but retain observation when reconciliation moves
  // the same element. Numeric updates do not rescan the whole sidebar.
  new MutationObserver(records => {
    const forget = node => {
      if (node.isConnected) return;
      observer.unobserve(node); watched.delete(node); visibleRows.delete(node);
    };
    for (const record of records) for (const node of record.removedNodes) {
      if (node.nodeType !== 1 || node.isConnected) continue;
      if (node.matches('.item')) forget(node);
      else if (node.matches('.group, .glist')) node.querySelectorAll('.item').forEach(forget);
    }
  }).observe(document.querySelector('#side'), {childList:true, subtree:true});
  document.querySelectorAll('#side .item').forEach(paint);
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
  globalThis.SessionDockSidebarResources = {paint, refresh};
  document.addEventListener('visibilitychange', () => {if (!document.hidden) {paintVisible(); void refresh();}});
  setInterval(() => {if (!document.hidden) {paintVisible(); void refresh();}}, 5000);
  toggle(enabled, false);
})();
