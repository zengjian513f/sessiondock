/* Shared resource accounting view. Collection and attribution stay in the service. */
(() => {
  'use strict';
  const fields = [
    ['cpu_cores', 'CPU', 'core'], ['gpu_count', 'GPU', 'gpu'],
    ['gpu_memory_bytes', '显存', 'bytes'], ['memory_pss_bytes', '内存 · PSS', 'bytes'],
    ['proc_storage_read_bytes_per_second', '存储层读取', 'rate'], ['proc_storage_write_bytes_per_second', '存储层写入', 'rate'],
    ['disk_read_bytes_per_second', '本地文件读取', 'rate'], ['disk_write_bytes_per_second', '本地文件写入', 'rate'],
    ['network_receive_bytes_per_second', 'TCP 接收', 'rate'], ['network_send_bytes_per_second', 'TCP 发送', 'rate'],
    ['nfs_read_bytes_per_second', 'NFS 读取', 'rate'], ['nfs_write_bytes_per_second', 'NFS 写入', 'rate'],
  ];
  const reasons = {unsupported: '采集端不支持', unavailable: '暂无采样', offline: '机器离线', stale: '采样已过期', partial: '部分覆盖', warming_up: '等待下一次采样'};
  const esc = value => String(value ?? '').replace(/[&<>"']/g, ch => ({'&':'&amp;', '<':'&lt;', '>':'&gt;', '"':'&quot;', "'":'&#39;'}[ch]));
  function number(value, unit) {
    if (unit === 'core') return `${value.toLocaleString('zh-CN', {maximumFractionDigits: 2})}<small> 核</small>`;
    if (unit === 'gpu') return `${value.toLocaleString('zh-CN')}<small> 张</small>`;
    const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
    let i = 0;
    while (value >= 1024 && i < units.length - 1) { value /= 1024; i++; }
    return `${value.toLocaleString('zh-CN', {maximumFractionDigits: i ? 1 : 0})}<small> ${units[i]}${unit === 'rate' ? '/s' : ''}</small>`;
  }
  function metric(raw, unit, fallback) {
    const item = typeof raw === 'number' ? {value: raw, status: 'ok'} : (raw || {});
    const usable = Number.isFinite(item.value) && !['unsupported', 'unavailable', 'offline', 'stale'].includes(item.status);
    const reason = item.reason || reasons[item.status] || fallback || '暂无采样';
    const note = /[\u3400-\u9fff]/.test(reason) ? reason : reasons[item.status] || '暂无采样';
    return {value: usable ? number(item.value, unit) : '—', note: usable ? (item.status === 'partial' ? '部分覆盖' : '') : note, partial: item.status === 'partial', reason: reason + (Number.isFinite(item.sampled_at) ? ` · 采样 ${new Date(item.sampled_at * 1000).toLocaleTimeString('zh-CN', {hour12:false})}` : '')};
  }
  function metrics(values, fallback) {
    return fields.map(([key, label, unit]) => {
      const item = metric(values?.[key], unit, fallback);
      return `<div class="sr-metric${item.partial ? ' sr-partial' : ''}" title="${esc(item.reason)}"><dt>${label}</dt><dd>${item.value}</dd>${item.note ? `<span class="sr-metric-note">${esc(item.note)}</span>` : ''}</div>`;
    }).join('');
  }
  function render(data) {
    const nodes = data.nodes || [];
    const stamp = data.sampled_at ? new Date(data.sampled_at * 1000).toLocaleTimeString('zh-CN', {hour12: false}) : '';
    return `<div class="sr-summary-head"><span>跨机器合计</span><span>${stamp ? `采样 ${esc(stamp)}` : '实时采样'}</span></div>
      <dl class="sr-metrics sr-totals">${metrics(data.totals?.metrics || data.totals)}</dl>
      <div class="sr-section-heading">执行机器 <span>${nodes.length}</span></div>
      ${nodes.length ? nodes.map(node => `<section class="sr-node"><div class="sr-node-head"><h3>${esc(node.node_name || node.node_id)}</h3><span class="sr-status${node.status === 'ok' ? ' sr-status-ok' : ''}">${esc(node.status === 'ok' ? '在线' : reasons[node.status] || node.reason || '状态未知')}</span></div><dl class="sr-metrics">${metrics(node.metrics, node.reason || reasons[node.status])}</dl></section>`).join('') : '<div class="sr-empty">尚未发现可归属的进程。未采集的数据不代表零占用。</div>'}
      <p class="sr-footnote">CPU 核数表示占用，异构机器之间不代表等同算力。PSS 内存约 30 秒、GPU 约 10 秒刷新。GPU 卡数表示使用的设备，不代表满卡算力。本地文件与 NFS 为同步文件读写量，含缓存命中，不等同于硬盘或 NFS RPC 流量；TCP 为应用收发量，不含 UDP、重传及 NFS 内核流量。将鼠标停在指标上可查看覆盖范围。</p>`;
  }
  let dialog, content, subtitle, currentUid = '', scope = 'direct', generation = 0, pending = false;
  let loader = async (uid, selectedScope) => {
    const path = `api/session/resources?${new URLSearchParams({uid, scope: selectedScope})}`;
    const response = await fetch(typeof appUrl === 'function' ? appUrl(path) : path);
    if (!response.ok) throw new Error(response.status === 404 ? '此服务尚未提供资源统计，请更新服务端。' : `资源统计暂不可用（${response.status}）`);
    return response.json();
  };
  function selection() {
    if (typeof S === 'undefined') return null;
    return S.sessions?.find(row => row.uid === S.sel) || null;
  }
  function ensure() {
    if (dialog) return;
    dialog = document.createElement('dialog');
    dialog.className = 'session-resources';
    dialog.setAttribute('aria-labelledby', 'sr-title');
    dialog.innerHTML = `<div class="sr-top"><div><h2 id="sr-title">会话资源</h2><p class="sr-subtitle"></p></div><button type="button" class="sr-close" aria-label="关闭资源面板">×</button></div>
      <div class="sr-controls"><div class="sr-scope" role="group" aria-label="资源归属范围"><button type="button" data-scope="direct" aria-pressed="true">直接占用</button><button type="button" data-scope="inclusive" aria-pressed="false">含发起任务</button></div><button type="button" class="sr-refresh" aria-label="刷新资源">刷新</button></div>
      <p class="sr-scope-help">仅统计直接归属于此会话的进程。</p><div class="sr-content" aria-live="polite"></div>`;
    document.body.append(dialog);
    content = dialog.querySelector('.sr-content');
    subtitle = dialog.querySelector('.sr-subtitle');
    dialog.querySelector('.sr-close').onclick = () => dialog.close();
    dialog.addEventListener('click', event => { if (event.target === dialog) dialog.close(); });
    dialog.addEventListener('close', () => { generation++; pending = false; });
    dialog.querySelector('.sr-refresh').onclick = () => refresh(true);
    dialog.querySelectorAll('[data-scope]').forEach(button => {
      button.onclick = () => {
        scope = button.dataset.scope;
        dialog.querySelectorAll('[data-scope]').forEach(item => item.setAttribute('aria-pressed', String(item === button)));
        dialog.querySelector('.sr-scope-help').textContent = scope === 'direct' ? '仅统计直接归属于此会话的进程。' : '包含经验证的 SSH 发起链及子任务；同一进程只计一次。';
        refresh(true);
      };
    });
  }
  async function refresh(force = false) {
    if (!dialog?.open || (pending && !force)) return;
    const selected = selection();
    if (!selected) { content.innerHTML = '<p class="sr-empty">请选择一个会话。</p>'; return; }
    const changed = currentUid !== selected.uid;
    currentUid = selected.uid;
    subtitle.textContent = selected.title || selected.sid || selected.uid;
    const ticket = ++generation;
    pending = true;
    if (changed || force || !content.innerHTML) content.innerHTML = '<p class="sr-empty">正在读取资源采样…</p>';
    try {
      const data = await loader(currentUid, scope);
      if (ticket !== generation || !dialog.open) return;
      content.innerHTML = render(data);
    } catch (error) {
      if (ticket === generation && dialog.open) content.innerHTML = `<p class="sr-empty sr-error">${esc(error.message || '资源统计暂不可用')}</p>`;
    } finally { if (ticket === generation) pending = false; }
  }
  function open() { ensure(); if (!dialog.open) dialog.showModal(); refresh(true); }
  function attach() {
    const actions = document.querySelector('#detail > .dhead .dhead-actions');
    if (!actions || actions.querySelector('[data-session-resources]')) return;
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'sr-open';
    button.dataset.sessionResources = '';
    button.textContent = '资源';
    button.setAttribute('aria-label', '查看会话资源');
    button.setAttribute('aria-haspopup', 'dialog');
    button.onclick = open;
    actions.prepend(button);
  }
  const detail = document.querySelector('#detail');
  if (detail) new MutationObserver(attach).observe(detail, {childList: true, subtree: true});
  attach();
  setInterval(() => { if (dialog?.open && !document.hidden) refresh(); }, 5000);
  window.SessionDockResources = {render, open, setLoader: fn => { loader = fn; }};
})();
