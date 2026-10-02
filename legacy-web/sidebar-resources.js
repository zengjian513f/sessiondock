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
  const scope = '当前会话直接归属进程，含 SSH 跨机任务，不含单独归属的子会话';
  let rows = new Map(), localNode = '', sampledAt = 0, partial = false, pending = false;
  const key = s => JSON.stringify([s.node_id || localNode, s.source, s.sid]);
  const digits = (n, places = 1) => n.toLocaleString('zh-CN', {maximumFractionDigits:places, useGrouping:false});
  function compact(value, unit) {
    if (unit === 'core') return value > 0 && value < .01 ? '<0.01' : digits(value, 2);
    if (unit === 'process' || unit === 'gpu') return digits(value, 0);
    if (!value) return '0';
    const units = ['B','K','M','G','T','P']; let i = 0;
    while (value >= 1024 && i < units.length - 1) {value /= 1024; i++;}
    return digits(value, value < 10 && i ? 1 : 0) + units[i] + (unit === 'rate' ? '/s' : '');
  }
  function paint(node) {
    const session = node._resourceSession, meta = node.querySelector('.m');
    if (!session || !meta) return;
    if (!enabled) { node.querySelector('.item-resources')?.remove(); return; }
    let panel = node.querySelector('.item-resources');
    if (!panel) {
      panel = document.createElement('div'); panel.className = 'item-resources';
      panel.setAttribute('aria-label', '会话资源');
      node.querySelector('.body').after(panel);
    }
    const age = Date.now()/1000 - sampledAt;
    const metrics = age >= -5 && age <= 15 && !node._resourceAgent ? rows.get(key(session)) : null;
    const cells = fields.map(([field, icon, label, unit]) => {
      const m = metrics?.[field];
      const valid = Number.isFinite(m?.value) && m.value >= 0 && ['ok','partial'].includes(m.status);
      const value = valid ? compact(m.value, unit) : '—';
      const detail = valid ? (unit === 'core' ? `${digits(m.value, 3)} 核` : unit === 'process' ? `${digits(m.value, 0)} 个` : unit === 'gpu' ? `${digits(m.value, 0)} 张` : `${digits(m.value, 0)} B${unit === 'rate' ? '/s' : ''}`)
        : node._resourceAgent ? '子代理未单独归属资源' : '暂无可归属的资源数据';
      const explanation = unit === 'process' ? '当前采样中直接归属该会话的进程，按机器及进程身份去重' : unit === 'gpu' ? '计算进程驻留的显卡张数，同一机器上的同一显卡只计一次；不代表计算利用率' : field === 'memory_pss_bytes' ? '共享内存按比例分摊；约 30 秒更新' : unit === 'rate' ? '常驻内核存储层计数，非缓存命中的文件读取量；延迟回写可能影响归属' : '1 表示占用一个逻辑 CPU 核';
      return {field, icon, label, value, tip:`${label}：${detail} · ${explanation} · ${scope}${partial || m?.status === 'partial' ? ' · 部分覆盖，缺失机器未计入' : ''}${node._resourceMeta ? ' · ' + node._resourceMeta : ''}`};
    });
    const signature = JSON.stringify([cells, node._resourceMeta]);
    if (panel.dataset.resourceSignature === signature) return;
    panel.dataset.resourceSignature = signature;
    panel.innerHTML = cells.map(c => `<span class="item-resource" data-resource="${c.field}" title="${esc(c.tip)}" role="img" aria-label="${esc(c.tip)}">${uiIcon(c.icon)}<span class="item-resource-value">${esc(c.value)}</span></span>`).join('');
  }
  function paintVisible() { document.querySelectorAll('#side .item').forEach(paint); }
  async function refresh() {
    if (!enabled || pending || document.hidden) return;
    pending = true;
    try {
      const response = await fetch(appUrl('api/resources/summary'), {signal:AbortSignal.timeout(4500)});
      if (!response.ok) throw new Error('resource summary unavailable');
      const data = await response.json();
      if (!Array.isArray(data.sessions) || !Number.isFinite(data.sampled_at)) throw new Error('invalid summary');
      localNode = data.node_id || ''; sampledAt = data.sampled_at; partial = !!data.partial;
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
