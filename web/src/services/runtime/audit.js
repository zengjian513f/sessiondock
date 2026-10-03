// Queue/batch/transit records stay raw in this scoped service.
export function createAuditService({fetch, appUrl, capabilities, network, pageId: AUDIT_PAGE_ID, buildId: BUILD_ID, selected}) {
let browserAuditQueue = [];
let browserAuditTimer = 0;
let browserAuditDue = 0;
let browserAuditRetryAt = 0;
let browserAuditSending = false;
let browserAuditFailCount = 0;
const AUDIT_BATCH_BYTES = 48 * 1024;
const AUDIT_BATCH_COUNT = 100;
const AUDIT_FLUSH_MS = 5000;
const AUDIT_CONTENT_LIMIT = 32 * 1024;

// 按 UTF-8 字节算：sendBeacon 的 64 KB 配额是字节数，中文一个字占 3 字节
const auditEncoder = new TextEncoder();
function auditEventBytes(event) {
  try { return auditEncoder.encode(JSON.stringify(event)).length; } catch { return AUDIT_BATCH_BYTES + 1; }
}

function spliceAuditBatch(queue, limit = AUDIT_BATCH_BYTES, count = AUDIT_BATCH_COUNT) {
  if (!queue.length) return [];
  if (auditEventBytes(queue[0]) > limit) return queue.splice(0, 1);
  const batch = [];
  let bytes = 0;
  while (queue.length && batch.length < count) {
    const size = auditEventBytes(queue[0]);
    if (bytes + size > limit) break;
    batch.push(queue.shift());
    bytes += size;
  }
  return batch;
}

function capAuditQueue() {
  if (browserAuditQueue.length > 500) browserAuditQueue.splice(0, browserAuditQueue.length - 500);
}

// Ordinary diagnostics share a batch across several polling cycles. Full
// batches and failures flush sooner; another info event never postpones a flush.
function scheduleBrowserAudit(delay = AUDIT_FLUSH_MS) {
  delay = Math.max(delay, browserAuditRetryAt - performance.now());
  const due = performance.now() + delay;
  if (browserAuditTimer && browserAuditDue <= due) return;
  clearTimeout(browserAuditTimer);
  browserAuditDue = due;
  browserAuditTimer = setTimeout(flushBrowserAudit, delay);
}

function browserAuditEvent(event, data = {}, content = null, fields = {}) {
  if (!capabilities.allows('audit')) return;
  try {
    // Rust stores metadata only; sending message/DOM/terminal bodies here
    // wastes bandwidth and prematurely fills the byte-limited audit batches.
    let auditContent = capabilities.config.backend === 'rust' ? null : content;
    let auditData = data;
    try {
      if (auditContent != null) {
        const serialized = JSON.stringify(auditContent);
        if (serialized.length > AUDIT_CONTENT_LIMIT) {
          auditContent = {truncated: true, bytes: serialized.length,
            head: serialized.slice(0, 8 * 1024)};
          auditData = {...data, content_truncated: true};
        }
      }
    } catch {
      auditContent = null;
    }
    browserAuditQueue.push({
      event, ts: new Date().toISOString(), uid: fields.uid ?? selected() ?? '',
      trace_id: fields.traceId || '', request_id: fields.requestId || '',
      connection_id: fields.connectionId || '', severity: fields.severity || 'info',
      data: auditData, content: auditContent,
    });
    capAuditQueue();
    scheduleBrowserAudit(browserAuditQueue.length >= AUDIT_BATCH_COUNT ? 0
      : fields.severity === 'error' ? 100 : AUDIT_FLUSH_MS);
  } catch { /* diagnostics never change UI behavior */ }
}

function auditPayload(events) {
  return JSON.stringify({
    page_id: AUDIT_PAGE_ID, uid: selected() || '', _build: BUILD_ID, events,
  });
}

async function flushBrowserAudit() {
  if (!capabilities.allows('audit')) return;
  clearTimeout(browserAuditTimer);
  browserAuditTimer = 0;
  if (network.paused) return;
  if (browserAuditSending || !browserAuditQueue.length) return;
  if (performance.now() < browserAuditRetryAt) { scheduleBrowserAudit(); return; }
  const events = spliceAuditBatch(browserAuditQueue);
  if (!events.length) return;
  browserAuditSending = true;
  try {
    const response = await fetch(appUrl('api/audit/browser'), {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json', 'X-SessionDock-Page': AUDIT_PAGE_ID,
        'X-SessionDock-Build': BUILD_ID,
      },
      body: auditPayload(events),
    });
    // 必须把响应体读完：没读的 fetch 响应在渲染进程里各占着一条 2 MiB 共享内存
    // 数据管道（一个 fd），直到 GC 才释放。这里每秒一条，渲染进程 1024 个 fd 的
    // 上限十几分钟就满，之后 GPU 命令缓冲区拿不到共享内存，整页在原生代码里卡死。
    await response.arrayBuffer().catch(() => {});
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    browserAuditFailCount = 0;
    browserAuditRetryAt = 0;
  } catch (error) {
    browserAuditRetryAt = performance.now() + AUDIT_FLUSH_MS;
    // 断网期间只是攒着，不算失败次数；连续三次在线失败才认定这一批本身有问题
    if (navigator.onLine) browserAuditFailCount += 1;
    if (browserAuditFailCount >= 3) {
      browserAuditFailCount = 0;
      const bytes = events.reduce((n, ev) => n + auditEventBytes(ev), 0);
      browserAuditQueue.push({
        event: 'audit.dropped', ts: new Date().toISOString(), uid: selected() ?? '',
        trace_id: '', request_id: '', connection_id: '', severity: 'info',
        data: {events: events.length, bytes,
          reason: String(error?.message || error || '').slice(0, 200)},
        content: null,
      });
      capAuditQueue();
    } else {
      browserAuditQueue.unshift(...events);
      if (browserAuditQueue.length > 500) browserAuditQueue.length = 500;
    }
  } finally {
    browserAuditSending = false;
    if (browserAuditQueue.length && !browserAuditTimer) {
      scheduleBrowserAudit(browserAuditQueue.length >= AUDIT_BATCH_COUNT ? 0 : AUDIT_FLUSH_MS);
    }
  }
}

// 卸载时整个页面只有 64 KB 的 beacon 配额（和 keepalive 同一个池），超出的包
// sendBeacon 直接返回 false。所以先把各事件的 content 丢掉保住事件本身，只给
// page.hidden 那条快照留 content；分两包发，合计不超过 60 KB。发不出去的留在
// 队列里，页面若只是进了 bfcache 还能补发。
function flushBrowserAuditBeacon() {
  if (network.paused) return;
  if (!capabilities.allows('audit')) return;
  clearTimeout(browserAuditTimer);
  browserAuditTimer = 0;
  if (!navigator.sendBeacon || !browserAuditQueue.length) return;
  const slim = browserAuditQueue.map(ev => ev.event === 'page.hidden' || ev.content == null ? ev
    : {...ev, content: null, data: {...ev.data, content_dropped: true}});
  browserAuditQueue = [];
  for (const budget of [40 * 1024, 20 * 1024]) {
    const events = spliceAuditBatch(slim, budget, 100);
    if (!events.length) break;
    const ok = navigator.sendBeacon(appUrl('api/audit/browser'),
      new Blob([auditPayload(events)], {type: 'application/json'}));
    if (!ok) { slim.unshift(...events); break; }
  }
  if (slim.length) {
    browserAuditQueue.unshift(...slim);
    capAuditQueue();
  }
}

  return {browserAuditEvent, flushBrowserAudit, flushBrowserAuditBeacon, auditPayload, get queue(){return browserAuditQueue},get sending(){return browserAuditSending},get failureCount(){return browserAuditFailCount},get queueLength() {return browserAuditQueue.length;}};
}
