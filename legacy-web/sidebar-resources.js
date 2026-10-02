/* Batch resource badges reuse the sidebar's metadata row and icon system. */
(() => {
  'use strict';
  const fields = [
    ['cpu_cores', 'resource-cpu', 'CPU 核数', 'core'],
    ['memory_pss_bytes', 'resource-memory', '内存占用（PSS）', 'bytes'],
    ['proc_storage_read_bytes_per_second', 'resource-read', '磁盘读取速度', 'rate'],
    ['proc_storage_write_bytes_per_second', 'resource-write', '磁盘写入速度', 'rate'],
  ];
  const scope = '当前会话直接归属进程，含 SSH 跨机任务，不含单独归属的子会话';
  let rows = new Map(), localNode = '', sampledAt = 0, partial = false, pending = false;
  const key = s => JSON.stringify([s.node_id || localNode, s.source, s.sid]);
  const digits = (n, places = 1) => n.toLocaleString('zh-CN', {maximumFractionDigits:places, useGrouping:false});
  function compact(value, unit) {
    if (unit === 'core') return value > 0 && value < .01 ? '<0.01' : digits(value, 2);
    if (!value) return '0';
    const units = ['B','K','M','G','T','P']; let i = 0;
    while (value >= 1024 && i < units.length - 1) {value /= 1024; i++;}
    return digits(value, value < 10 && i ? 1 : 0) + units[i] + (unit === 'rate' ? '/s' : '');
  }
  function paint(node) {
    const session = node._resourceSession, meta = node.querySelector('.m');
    if (!session || !meta) return;
    const age = Date.now()/1000 - sampledAt;
    const metrics = age >= -5 && age <= 15 && !node._resourceAgent ? rows.get(key(session)) : null;
    const cells = fields.map(([field, icon, label, unit]) => {
      const m = metrics?.[field];
      const valid = Number.isFinite(m?.value) && m.value >= 0 && ['ok','partial'].includes(m.status);
      const value = valid ? compact(m.value, unit) : '—';
      const detail = valid ? (unit === 'core' ? `${digits(m.value, 3)} 核` : `${digits(m.value, 0)} B${unit === 'rate' ? '/s' : ''}`)
        : node._resourceAgent ? '子代理未单独归属资源' : '暂无可归属的资源数据';
      const explanation = field === 'memory_pss_bytes' ? '共享内存按比例分摊；约 30 秒更新' : unit === 'rate' ? '常驻内核存储层计数，非缓存命中的文件读取量；延迟回写可能影响归属' : '1 表示占用一个逻辑 CPU 核';
      return {field, icon, label, value, tip:`${label}：${detail} · ${explanation} · ${scope}${partial || m?.status === 'partial' ? ' · 部分覆盖，缺失机器未计入' : ''}${node._resourceMeta ? ' · ' + node._resourceMeta : ''}`};
    });
    const signature = JSON.stringify([cells, node._resourceMeta]);
    if (meta.dataset.resourceSignature === signature && meta.querySelectorAll('.item-resource').length === 4) return;
    meta.dataset.resourceSignature = signature;
    meta.classList.add('item-resource-line');
    meta.title = [node._resourceMeta, scope].filter(Boolean).join(' · ');
    meta.innerHTML = `<span class="item-resource-meta" title="${esc(node._resourceMeta || '')}">${esc(node._resourceMeta || '')}</span><span class="item-resources">` + cells.map(c => `<span class="item-resource" data-resource="${c.field}" title="${esc(c.tip)}" role="img" aria-label="${esc(c.tip)}">${uiIcon(c.icon)}<span class="item-resource-value">${esc(c.value)}</span></span>`).join('') + '</span>';
  }
  function paintVisible() { document.querySelectorAll('#side .item').forEach(paint); }
  async function refresh() {
    if (pending || document.hidden) return;
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
  globalThis.SessionDockSidebarResources = {paint, refresh};
  document.addEventListener('visibilitychange', () => {if (!document.hidden) {paintVisible(); void refresh();}});
  setInterval(() => {if (!document.hidden) {paintVisible(); void refresh();}}, 5000);
  paintVisible(); void refresh();
})();
