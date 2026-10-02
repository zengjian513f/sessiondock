/* Shared resource accounting view. Collection and attribution stay in the service. */
(() => {
  'use strict';
  const fields = [
    ['cpu_cores', 'CPU', 'core'], ['gpu_count', 'GPU', 'gpu'],
    ['gpu_memory_bytes', '显存', 'bytes'], ['memory_pss_bytes', '内存 · PSS', 'bytes'],
    ['memory_bandwidth_bytes_per_second', '内存带宽', 'rate'],
    ['proc_storage_read_bytes_per_second', '存储层读取', 'rate'], ['proc_storage_write_bytes_per_second', '存储层写入', 'rate'],
    ['disk_read_operations_per_second', '本地读次数', 'ops'], ['disk_write_operations_per_second', '本地写次数', 'ops'],
    ['nfs_read_operations_per_second', 'NFS 读次数', 'ops'], ['nfs_write_operations_per_second', 'NFS 写次数', 'ops'],
    ['disk_read_bytes_per_second', '本地文件读取', 'rate'], ['disk_write_bytes_per_second', '本地文件写入', 'rate'],
    ['network_receive_bytes_per_second', 'TCP 接收', 'rate'], ['network_send_bytes_per_second', 'TCP 发送', 'rate'],
    ['nfs_read_bytes_per_second', 'NFS 读取', 'rate'], ['nfs_write_bytes_per_second', 'NFS 写入', 'rate'],
  ];
  const descriptions = {
    disk_read_operations_per_second: '60 秒临时探测：本地普通文件成功读取次数，含缓存命中；不是硬盘物理 IOPS，不含内存映射、io_uring 和 splice。',
    disk_write_operations_per_second: '60 秒临时探测：本地普通文件成功写入次数；不是硬盘物理 IOPS，不含内存映射、io_uring 和 splice。',
    nfs_read_operations_per_second: '60 秒临时探测：NFS 文件成功读取次数，含缓存命中；不是远程 RPC 次数，不含内存映射、io_uring 和 splice。',
    nfs_write_operations_per_second: '60 秒临时探测：NFS 文件成功写入次数；不是远程 RPC 次数，不含内存映射、io_uring 和 splice。',
    cpu_cores: '占用的逻辑 CPU 核数；不同机器的核数不代表相同算力。',
    gpu_count: '使用到的计算设备数，同机按设备去重；不代表独占或满卡算力。约 10 秒更新。',
    gpu_memory_bytes: '计算进程的显存占用；不含纯图形任务，共享计算服务可能无法细分到工作进程。约 10 秒更新。',
    memory_bandwidth_bytes_per_second: '常驻硬件监控，约 5 秒更新；按会话统计总内存流量，未拆分读写。子会话依标签范围汇总；不代表逐进程带宽，短命任务可能漏计。',
    memory_pss_bytes: '按比例分摊共享内存后的驻留内存，不用普通驻留内存替代缺失值。约 30 秒更新。',
    proc_storage_read_bytes_per_second: '进程在存储层引起的读取量，与缓存命中的文件读取量不同。',
    proc_storage_write_bytes_per_second: '进程在存储层引起的写入量；延迟回写可能影响归属和时间。',
    disk_read_bytes_per_second: '本地普通文件的同步逻辑读取量，含缓存命中；不含 NFS、内存映射和异步直接通道。',
    disk_write_bytes_per_second: '本地普通文件的同步逻辑写入量，不等同于硬盘实际写入；不含 NFS、内存映射和异步直接通道。',
    network_receive_bytes_per_second: '应用收到的 TCP 数据，不含 UDP、RDMA、重传或 NFS 内核流量。',
    network_send_bytes_per_second: '应用发送的 TCP 数据，不含 UDP、RDMA、重传或 NFS 内核流量。',
    nfs_read_bytes_per_second: 'NFS 文件的同步逻辑读取量，含缓存命中；不是实际远程请求或网卡流量。',
    nfs_write_bytes_per_second: 'NFS 文件的同步逻辑写入量；不含完整远程请求、重传及协议开销。',
  };
  const reasons = {unsupported: '采集端不支持', unavailable: '暂无采样', offline: '机器离线', stale: '采样已过期', partial: '部分覆盖', warming_up: '等待下一次采样'};
  const esc = value => String(value ?? '').replace(/[&<>"']/g, ch => ({'&':'&amp;', '<':'&lt;', '>':'&gt;', '"':'&quot;', "'":'&#39;'}[ch]));
  function number(value, unit) {
    if (unit === 'ops') return `${value.toLocaleString('zh-CN', {maximumFractionDigits: 1})}<small> 次/s</small>`;
    if (unit === 'core') return `${value.toLocaleString('zh-CN', {maximumFractionDigits: 2})}<small> 核</small>`;
    if (unit === 'gpu') return `${value.toLocaleString('zh-CN')}<small> 张</small>`;
    const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
    let i = 0;
    while (value >= 1024 && i < units.length - 1) { value /= 1024; i++; }
    return `${value.toLocaleString('zh-CN', {maximumFractionDigits: i ? 1 : 0})}<small> ${units[i]}${unit === 'rate' ? '/s' : ''}</small>`;
  }
  function metrics(values, fallback) {
    return fields.map(([key, label, unit]) => {
      const raw = values?.[key];
      const item = typeof raw === 'number' ? {value: raw, status: 'ok'} : (raw || {});
      const usable = Number.isFinite(item.value) && !['unsupported', 'unavailable', 'offline', 'stale', 'warming_up'].includes(item.status);
      const detail = /[\u3400-\u9fff]/.test(item.reason || '') ? item.reason : '';
      const status = reasons[item.status] || (!usable ? reasons[fallback] || '暂无采样' : '');
      const stamp = Number.isFinite(item.sampled_at) ? `采样 ${new Date(item.sampled_at * 1000).toLocaleTimeString('zh-CN', {hour12:false})}` : '';
      const tip = [descriptions[key], status, detail, stamp].filter(Boolean).join(' · ');
      return `<div class="sr-metric${item.status === 'partial' ? ' sr-partial' : ''}" tabindex="0" title="${esc(tip)}"><dt>${label}</dt><dd>${usable ? number(item.value, unit) : '—'}</dd></div>`;
    }).join('');
  }
  const probeTip = '在关联机器上临时开启逐次调用的文件 I/O、NFS 和网络测量，会增加采集开销，其他受监控会话也共享探测；停止会关闭这些机器的共享探测。60 秒后自动停止，关闭页面也不影响到期停止。';
  const probeStates = {off:'未开启', starting:'正在启动', active:'探测中', stopping:'正在停止', failed:'探测失败', unsupported:'不支持探测'};
  function diagnostic(node) {
    const item = node.diagnostic;
    if (!item) return '';
    const seconds = Math.max(0, Math.ceil(Number(item.remaining_seconds) || 0));
    return `<span class="sr-diagnostic${item.state === 'failed' ? ' sr-error' : ''}" data-probe-state="${esc(item.state)}" data-probe-seconds="${seconds}" title="${esc(item.error || probeTip)}">${esc(probeStates[item.state] || '探测状态未知')}${item.state === 'active' ? ` · ${seconds} 秒` : ''}${item.error ? ` · ${esc(item.error)}` : ''}</span>`;
  }
  function updateProbe(data) {
    const nodes = data.nodes || [];
    const running = nodes.some(node => ['starting', 'active', 'stopping'].includes(node.diagnostic?.state));
    const button = dialog.querySelector('.sr-probe');
    button.textContent = running ? '停止探测' : '探测 60 秒';
    button.dataset.enabled = String(running);
    button.disabled = probePending || !nodes.length || nodes.every(node => node.diagnostic?.state === 'unsupported');
  }
  function render(data) {
    const nodes = data.nodes || [];
    const stamp = data.sampled_at ? new Date(data.sampled_at * 1000).toLocaleTimeString('zh-CN', {hour12: false}) : '';
    return `<div class="sr-summary-head"><span title="仅合计当前会话相关机器的已知数据；缺失值不代表零。同一进程只计一次。">${nodes.length > 1 ? '跨机器合计' : '合计'}</span><span>${stamp ? `采样 ${esc(stamp)}` : '实时采样'}</span></div>
      <dl class="sr-metrics sr-totals">${metrics(data.totals?.metrics || data.totals)}</dl>
      <div class="sr-section-heading">执行机器 <span>${nodes.length}</span></div>
      ${nodes.length ? nodes.map(node => `<section class="sr-node"><div class="sr-node-head"><h3>${esc(node.node_name || node.node_id)}</h3><span class="sr-status${node.status === 'ok' ? ' sr-status-ok' : ''}">${esc(node.status === 'ok' ? '在线' : reasons[node.status] || '状态未知')}</span>${diagnostic(node)}</div><dl class="sr-metrics">${metrics(node.metrics, node.status)}</dl></section>`).join('') : '<div class="sr-empty" title="未采集的数据不代表零占用。">暂无关联进程</div>'}`;
  }
  let dialog, content, subtitle, currentUid = '', scope = 'inclusive', generation = 0, pending = false, probePending = false;
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
    dialog.innerHTML = `<div class="sr-top"><div><h2 id="sr-title" tabindex="0" title="按实际执行机器统计已归属的进程，同一进程只计一次。共享 CLI 内的子代理开销无法仅凭进程树精确拆分。">会话资源</h2><p class="sr-subtitle"></p></div><div class="sr-actions"><button type="button" class="sr-refresh" aria-label="刷新资源">刷新</button><button type="button" class="sr-close" aria-label="关闭资源面板">×</button></div></div>
      <div class="sr-toolbar"><div class="sr-scopes" role="group" aria-label="资源统计范围">
        <button type="button" data-scope="direct" aria-pressed="false" title="仅统计归属当前会话的进程，包含 SSH 远端命令；不包含已单独归属子会话的进程。">仅当前会话</button>
        <button type="button" data-scope="inclusive" aria-pressed="true" title="包含当前会话及已确认关联的子会话进程，无论在本机还是远端。子代理需要有可识别的进程归属。">包含子会话</button>
      </div>
      <div class="sr-probe-controls"><button type="button" class="sr-probe" disabled title="${esc(probeTip)}">探测 60 秒</button></div></div><span class="sr-probe-error sr-error" role="status"></span>
      <div class="sr-content" aria-live="polite"></div>`;
    document.body.append(dialog);
    content = dialog.querySelector('.sr-content');
    subtitle = dialog.querySelector('.sr-subtitle');
    dialog.querySelector('.sr-close').onclick = () => dialog.close();
    dialog.addEventListener('click', event => { if (event.target === dialog) dialog.close(); });
    dialog.addEventListener('close', () => { generation++; pending = false; });
    dialog.querySelectorAll('[data-scope]').forEach(button => {
      button.onclick = () => {
        scope = button.dataset.scope;
        dialog.querySelector('.sr-probe-error').textContent = '';
        dialog.querySelectorAll('[data-scope]').forEach(tab => tab.setAttribute('aria-pressed', String(tab.dataset.scope === scope)));
        refresh(true);
      };
    });
    dialog.querySelector('.sr-refresh').onclick = () => refresh(true);
    dialog.querySelector('.sr-probe').onclick = async () => {
      if (probePending) return;
      const selected = selection();
      if (!selected) return;
      const uid = selected.uid, selectedScope = scope;
      const button = dialog.querySelector('.sr-probe');
      const enabled = button.dataset.enabled !== 'true';
      const errorLabel = dialog.querySelector('.sr-probe-error');
      probePending = true;
      button.disabled = true;
      errorLabel.textContent = '';
      try {
        const path = 'api/session/resources/probe';
        const response = await fetch(typeof appUrl === 'function' ? appUrl(path) : path, {
          method: 'POST', headers: {'Content-Type': 'application/json'},
          body: JSON.stringify({uid, scope: selectedScope, enabled}),
        });
        if (!response.ok) throw new Error(`探测请求失败（${response.status}）`);
        const result = await response.json().catch(() => null);
        const failures = (result?.nodes || []).filter(node => node.ok === false);
        if (failures.length && dialog.open && selection()?.uid === uid && scope === selectedScope) {
          errorLabel.textContent = failures.map(node => `${node.node_name || node.node_id || '关联机器'}：${node.error || '探测请求失败'}`).join('；');
        }
      } catch (error) {
        if (dialog.open && selection()?.uid === uid && scope === selectedScope) errorLabel.textContent = error.message || '探测请求失败';
      } finally {
        probePending = false;
        if (dialog.open) await refresh(true);
      }
    };
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
    if (changed || force) dialog.querySelector('.sr-probe').disabled = true;
    if (changed) dialog.querySelector('.sr-probe-error').textContent = '';
    if (changed || force || !content.innerHTML) content.innerHTML = '<p class="sr-empty">正在读取资源采样…</p>';
    try {
      const data = await loader(currentUid, scope);
      if (ticket !== generation || !dialog.open) return;
      content.innerHTML = render(data);
      updateProbe(data);
    } catch (error) {
      if (ticket === generation && dialog.open) { dialog.querySelector('.sr-probe').disabled = true; content.innerHTML = `<p class="sr-empty sr-error">${esc(error.message || '资源统计暂不可用')}</p>`; }
    } finally { if (ticket === generation) pending = false; }
  }
  function open() { ensure(); if (!dialog.open) dialog.showModal(); refresh(true); }
  function attach() {
    const actions = document.querySelector('#detail > .dhead .dhead-actions');
    if (!actions || actions.querySelector('[data-session-resources]')) return;
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'iconbtn sr-open';
    button.dataset.sessionResources = '';
    button.innerHTML = '<svg class="ui-icon" viewBox="0 0 16 16" aria-hidden="true"><path d="M2 9h3l2-6 3 10 2-6h2" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/></svg>';
    button.title = '查看会话资源';
    button.setAttribute('aria-label', '查看会话资源');
    button.setAttribute('aria-haspopup', 'dialog');
    button.onclick = open;
    actions.prepend(button);
  }
  const detail = document.querySelector('#detail');
  if (detail) new MutationObserver(attach).observe(detail, {childList: true, subtree: true});
  attach();
  setInterval(() => {
    if (!dialog?.open || document.hidden) return;
    dialog.querySelectorAll('[data-probe-state="active"]').forEach(label => {
      const seconds = Math.max(0, Number(label.dataset.probeSeconds) - 1);
      label.dataset.probeSeconds = String(seconds);
      label.textContent = seconds ? `探测中 · ${seconds} 秒` : '等待探测状态更新';
    });
  }, 1000);
  setInterval(() => { if (dialog?.open && !document.hidden) refresh(); }, 5000);
  window.SessionDockResources = {render, open, setLoader: fn => { loader = fn; }};
})();
