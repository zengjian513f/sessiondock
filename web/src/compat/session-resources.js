/* Shared resource accounting view. Collection and attribution stay in the service. */
(() => {
  'use strict';
  const fields = [
    ['cpu_cores', 'CPU', 'core'], ['gpu_count', 'GPU', 'gpu'],
    ['gpu_memory_bytes', '显存', 'bytes'], ['memory_pss_bytes', '内存 · PSS', 'bytes'],
    ['process_count', '进程数', 'count'],
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
    process_count: '当前采样中已归属的进程数，按机器和进程身份去重。',
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
    if (unit === 'count') return `${value.toLocaleString('zh-CN')}<small> 个</small>`;
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
  const probeStates = {off:'未探测', starting:'正在启动', active:'探测中', stopping:'正在停止', failed:'探测失败', unsupported:'不支持探测'};
  function diagnostic(node) {
    const item = node.diagnostic;
    if (!item) return '';
    const seconds = Math.max(0, Math.ceil(Number(item.remaining_seconds) || 0));
    return `<span class="sr-diagnostic${item.state === 'failed' ? ' sr-error' : ''}" data-probe-state="${esc(item.state)}" data-probe-seconds="${seconds}">${esc(probeStates[item.state] || '探测状态未知')}${item.error ? ` · ${esc(item.error)}` : ''}</span>`;
  }
  let probeSupported = false, observedProbeState = 'off';
  function updateProbe(data) {
    const nodes = data.nodes || [];
    probeSupported = nodes.some(node => node.diagnostic && node.diagnostic.state !== 'unsupported');
    observedProbeState = nodes.some(node => node.diagnostic?.state === 'active') ? 'active'
      : nodes.some(node => node.diagnostic?.state === 'starting') ? 'starting'
      : nodes.some(node => node.diagnostic?.state === 'failed') ? 'failed' : 'off';
    paintProbeStatus();
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
  let requestedSession = null;
  function selection() {
    if (requestedSession) return requestedSession;
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
      <div class="sr-probe-controls"><span class="sr-probe" role="status">未探测</span></div></div><span class="sr-probe-error sr-error" role="status"></span>
      <div class="sr-content" aria-live="polite"></div>`;
    document.body.append(dialog);
    content = dialog.querySelector('.sr-content');
    subtitle = dialog.querySelector('.sr-subtitle');
    dialog.querySelector('.sr-close').onclick = () => dialog.close();
    dialog.addEventListener('click', event => { if (event.target === dialog) dialog.close(); });
    dialog.addEventListener('close', () => { generation++; pending = false; void reconcileProbe(); });
    dialog.querySelectorAll('[data-scope]').forEach(button => {
      button.onclick = () => {
        scope = button.dataset.scope;
        dialog.querySelector('.sr-probe-error').textContent = '';
        dialog.querySelectorAll('[data-scope]').forEach(tab => tab.setAttribute('aria-pressed', String(tab.dataset.scope === scope)));
        refresh(true);
      };
    });
    dialog.querySelector('.sr-refresh').onclick = () => refresh(true);
  }

  let lease = null, leaseId = '', lastSentActivity = 0, lastProbeRequest = 0, pageGone = false;
  function lastActivity() { return globalThis.SessionDockSleep?.lastActivity || 0; }
  function wantsProbe() {
    return !!dialog?.open && !pageGone && !globalThis.SessionDockSleep?.sleeping
      && Date.now() - lastActivity() < 60000 && probeSupported;
  }
  function paintProbeStatus() {
    if (!dialog) return;
    const state = !wantsProbe() ? 'off' : observedProbeState === 'active' ? 'active'
      : probePending ? 'starting' : observedProbeState;
    const label = dialog.querySelector('.sr-probe');
    label.dataset.state = state;
    label.textContent = probeStates[state] || '未探测';
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
      const response = await fetch(typeof appUrl === 'function' ? appUrl(path) : path, {
        method:'POST', headers:{'Content-Type':'application/json'}, keepalive:true, signal:AbortSignal.timeout(8000),
        body:JSON.stringify({...target, enabled, lease_seconds:Math.max(1, Math.min(60, Math.ceil((activity+60000-now)/1000)))})
      });
      if (!response.ok) throw new Error(`探测请求失败（${response.status}）`);
      const result = await response.json();
      if (enabled) { lease = target; lastSentActivity = activity; }
      const failures = (result.nodes || []).filter(node => node.ok === false);
      if (dialog?.open) dialog.querySelector('.sr-probe-error').textContent = failures.map(node => `${node.node_name || '关联机器'}：${node.error || '探测请求失败'}`).join('；');
      if (enabled && failures.length && failures.length === (result.nodes || []).length) observedProbeState = 'failed';
    } catch (error) {
      // A lost response can still have installed a lease. Retain its identity
      // so closing/idle can release it; the server also enforces its deadline.
      if (enabled) { lease = target; lastSentActivity = activity; }
      observedProbeState = 'failed';
      if (dialog?.open) dialog.querySelector('.sr-probe-error').textContent = error.message || '探测请求失败';
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
    if (!selected) { content.innerHTML = '<p class="sr-empty">请选择一个会话。</p>'; return; }
    const changed = currentUid !== selected.uid;
    currentUid = selected.uid;
    subtitle.textContent = selected.title || selected.sid || selected.uid;
    const ticket = ++generation;
    pending = true;
    if (changed) dialog.querySelector('.sr-probe-error').textContent = '';
    if (changed || force || !content.innerHTML) content.innerHTML = '<p class="sr-empty">正在读取资源采样…</p>';
    try {
      const data = await loader(currentUid, scope);
      if (ticket !== generation || !dialog.open) return;
      content.innerHTML = render(data);
      updateProbe(data);
      void reconcileProbe();
    } catch (error) {
      if (ticket === generation && dialog.open) { content.innerHTML = `<p class="sr-empty sr-error">${esc(error.message || '资源统计暂不可用')}</p>`; }
    } finally { if (ticket === generation) pending = false; }
  }
  function open(session) {
    requestedSession = session?.uid ? {uid:session.uid, title:session.title, sid:session.sid} : null;
    ensure();
    leaseId = crypto.randomUUID();
    probeSupported = false; observedProbeState = 'off'; lastProbeRequest = 0;
    if (!dialog.open) dialog.showModal();
    refresh(true);
  }
  setInterval(() => { void reconcileProbe(); }, 1000);
  setInterval(() => { if (dialog?.open && !document.hidden) refresh(); }, 5000);
  addEventListener('pagehide', () => { pageGone = true; void reconcileProbe(); });
  addEventListener('pageshow', () => { pageGone = false; });
  window.SessionDockResources = {render, open, setLoader: fn => { loader = fn; }};
})();
