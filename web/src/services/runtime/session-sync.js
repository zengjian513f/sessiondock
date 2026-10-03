import * as Conversation from '../../migration/conversation'
import {viewKey} from '../../domain/runtime/messages.js'
import {transientReadFailure,retryDelay} from '../../domain/runtime/read-failures.js'

// This controller owns fetch deduplication, checkpoint recovery and SSE lifetime.
// Native snapshots and in-flight promises never enter a reactive view store.
export function createSessionSync(environment, state, messageCache, reader, rendering, audit) {
  const {capabilities, network, appUrl, pageId: AUDIT_PAGE_ID} = environment;
  const {selection, unread} = state;
  const {cache} = messageCache;
  const cachePut = (...args) => messageCache.cachePut(...args);
  const fetchMessages = (...args) => reader.fetchMessages(...args);
  const {applyDiff, renderSession} = rendering;
  const {browserAuditEvent, browserStateSnapshot, scheduleBrowserSnapshot} = audit;
  const timing = {syncStallMs: 12000};
  const $ = selector => document.querySelector(selector);
const syncingViews = new Map();
// Migration-only failures stop background retries for this exact view. Keep
// the last good snapshot; only a successful explicit HTTP retry clears them.
const migrationReadFailures = new Map();
const migrationReadRetries = new Map();
const migrationReadProbes = new Map();

function migrationReadPaused(uid, agent = null) {
  return capabilities.config.backend === 'rust'
    && migrationReadFailures.has(viewKey(uid, agent));
}

/** 瞬时失败只影响这一次请求：网络层错误、中止、408/429、5xx（501 除外）。
 *  它们走退避重试，不暂停视图、不弹横幅、不关 SSE。 */
function renderMigrationReadFailure(uid, agent = null) {
  if (capabilities.config.backend !== 'rust' || selection.sel !== uid || selection.agent !== agent) return;
  const detail = $('#detail'); if (!detail) return;
  const failure = migrationReadFailures.get(viewKey(uid,agent));
  if (!failure) delete detail.dataset.migrationStale;
  else detail.dataset.migrationStale = 'true';
  Conversation.readFailure(detail,uid,agent,cache.has(viewKey(uid,agent)) ? failure : null);
}

/** 不可恢复失败的登记：暂停该视图的后台读取与 SSE，保留先前快照。 */
function reportMigrationReadFailure(uid, agent, error) {
  if (capabilities.config.backend !== 'rust') return null;
  const failure = {message: String(error?.message || error?.error || '无法确认最新历史，请重试。'),
    status: Number(error?.status) || 0, code: String(error?.code || '')};
  migrationReadFailures.set(viewKey(uid, agent), failure);
  if (selection.sel === uid && selection.agent === agent && _esUid === uid) closeWatch();
  renderMigrationReadFailure(uid, agent);
  return failure;
}

/** 请求失败的分流：瞬时失败不登记（返回 null，由调用方按退避重试），
 *  其余交给 reportMigrationReadFailure。 */
function reportReadFailure(uid, agent, error) {
  if (capabilities.config.backend !== 'rust') return null;
  if (transientReadFailure(error)) return null;
  return reportMigrationReadFailure(uid, agent, error);
}

async function retryMigrationRead(uid, agent = null) {
  if (capabilities.config.backend !== 'rust') return false;
  const key = viewKey(uid, agent);
  if (migrationReadRetries.has(key)) return migrationReadRetries.get(key);
  const task = (async () => {
    // Let an already-started request finish before authorizing fresh state.
    await Promise.allSettled([syncingViews.get(key), migrationReadProbes.get(key)].filter(Boolean));
    const failure = migrationReadFailures.get(key);
    if (!failure) return false;
    failure.retrying = true;
    renderMigrationReadFailure(uid, agent);
    const ac = new AbortController();
    const timer = setTimeout(() => ac.abort(), timing.syncStallMs);
    try {
      const {data, bytes} = await fetchMessages(uid, {agent, windowed: true, signal: ac.signal});
      if (!data?.meta || !data.version || !Array.isArray(data.messages)
          || !Number.isFinite(data.end)) throw new Error('服务端返回了无效的会话快照');
      cachePut(key, {meta: data.meta, msgs: data.messages, version: data.version,
        end: data.end, anchor: data.anchor, activity: data.activity, bytes,
        prompt: data.prompt || null, cli: data.cli ?? null, total: data.message_total, partial: data.partial || null});
      unread.cursors.set(key, {end: data.end, head: data.version.head, anchor: data.anchor});
      migrationReadFailures.delete(key);
      if (selection.sel === uid && selection.agent === agent) {
        await renderSession(data.meta, data.messages, data.activity, {startWatch: false});
        renderMigrationReadFailure(uid, agent);
        if (selection.sel === uid && selection.agent === agent) watchSession(uid, agent);
      }
      return true;
    } catch (error) {
      reportMigrationReadFailure(uid, agent, error);
      return false;
    } finally { clearTimeout(timer); }
  })().finally(() => {
    if (migrationReadRetries.get(key) === task) migrationReadRetries.delete(key);
  });
  migrationReadRetries.set(key, task);
  return task;
}

/** 服务端明确的 migration-error 事件：不可恢复时暂停并保留快照。
 *  原生文件并发增长等情况也会以 migration-error 携带 503；它仍是瞬时失败，
 *  留给随后到来的 es.onerror 按退避策略重开。 */
function pauseMigrationWatch(es, uid, agent, error = null) {
  if (capabilities.config.backend !== 'rust' || _es !== es
      || _esUid !== uid || selection.sel !== uid || selection.agent !== agent) return false;
  if (!error) return false;
  if (transientReadFailure(error)) return false;
  reportMigrationReadFailure(uid, agent, error);
  return true;
}

/** EventSource 藏起了 HTTP 拒绝的正文。流从未打开就失败时，用一次增量读取
 *  探明原因：不可恢复（501 等）就暂停并给出理由；瞬时失败或读取成功则说明
 *  只是流本身没建起来，交回退避重连。探测不改快照、不重开流。 */
function probeWatchRejection(uid, agent) {
  if (capabilities.config.backend !== 'rust') return Promise.resolve(false);
  const key = viewKey(uid, agent);
  const current = migrationReadProbes.get(key);
  if (current) return current;
  const task = (async () => {
    const ac = new AbortController();
    const timer = setTimeout(() => ac.abort(), timing.syncStallMs);
    try {
      const entry = cache.get(key);
      await fetchMessages(uid, {agent, start: entry?.end, head: entry?.version?.head,
        anchor: entry?.anchor, signal: ac.signal});
      return false;
    } catch (reason) {
      return !!reportReadFailure(uid, agent, reason);
    } finally { clearTimeout(timer); }
  })().finally(() => {
    if (migrationReadProbes.get(key) === task) migrationReadProbes.delete(key);
  });
  migrationReadProbes.set(key, task);
  return task;
}

function syncSession(uid, agent = selection.agent) {
  const key = viewKey(uid, agent);
  if (migrationReadPaused(uid, agent)) return Promise.resolve(0);
  const e = cache.get(key);
  if (!e) return Promise.resolve(0);
  const current = syncingViews.get(key);
  if (current) return current;
  const task = (async () => {
    const ac = new AbortController();
    let stallTimer = null;
    const keepAlive = () => {
      clearTimeout(stallTimer);
      stallTimer = setTimeout(() => ac.abort(), timing.syncStallMs);
    };
    try {
      keepAlive();
      const { data, bytes } = await fetchMessages(uid, {
        agent, start: e.end, head: e.version.head, anchor: e.anchor,
        signal: ac.signal, onActivity: keepAlive });
      return await applyDiff(uid, data, bytes, agent);
    } catch (error) {
      // 瞬时失败不登记：tickSync 会按自适应间隔再来，SSE 也照常重连。
      reportReadFailure(uid, agent, error);
      return 0;
    } finally {
      clearTimeout(stallTimer);
    }
  })().finally(() => {
    if (syncingViews.get(key) === task) syncingViews.delete(key);
  });
  syncingViews.set(key, task);
  return task;
}

// ---- 服务端推送 ----
// 服务端盯着会话文件, 一变就把 diff 推过来, 不用客户端反复问。
let _es = null, _esUid = null, _esRetry = null;
let _esRetryAttempt = 0;   // 连续未成功打开的次数，决定重连退避
const diffRecoveries = new Map();

/** 游标冲突或队列先于正文确认时，从当前已接受游标重新取一次并重建 watch。 */
function scheduleDiffRecovery(uid, agent = null) {
  const key = viewKey(uid, agent);
  if (diffRecoveries.has(key)) return;
  const timer = setTimeout(async () => {
    try {
      // 若冲突发生时已有主动拉取在途，先等旧请求收尾，再保证至少发出
      // 一次基于最新本地游标的新请求。
      const inFlight = syncingViews.get(key);
      if (inFlight) await inFlight;
      if (!cache.has(key)) return;
      await syncSession(uid, agent);
      if (selection.sel === uid && selection.agent === agent) watchSession(uid, agent);
    } finally {
      if (diffRecoveries.get(key) === timer) diffRecoveries.delete(key);
    }
  }, 0);
  diffRecoveries.set(key, timer);
}

function watchSession(uid, agent = selection.agent) {
  closeWatch();
  if (network.paused) return;
  if (migrationReadPaused(uid, agent)) { renderMigrationReadFailure(uid, agent); return; }
  const e = cache.get(viewKey(uid, agent));
  if (!e || !window.EventSource) return;
  // watch 从当前缓存游标开始，建立过程中不需要 tickSync 立刻再发一条相同
  // 增量请求。否则 CONNECTING 尚未变 OPEN 的几百毫秒会产生一次竞争包。
  if (selection.sel === uid && selection.agent === agent) selection.lastSync = Date.now();
  const connectionId = globalThis.crypto?.randomUUID?.()
    || `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  const p = new URLSearchParams({ uid, start: e.end, head: e.version.head,
    anchor: e.anchor || '', page: AUDIT_PAGE_ID, connection: connectionId });
  if (agent) p.set('agent', agent);
  const es = new EventSource(appUrl('api/watch?' + p));
  _es = es;
  _esUid = uid;
  es.__sessiondockConnectionId = connectionId;
  let received = 0;
  browserAuditEvent('sse.connecting', {start: e.end, agent: agent || ''}, null,
    {uid, connectionId});
  let opened = false;
  es.onopen = () => {
    opened = true;
    if (_es === es) _esRetryAttempt = 0;
    browserAuditEvent('sse.opened', {ready_state: es.readyState}, null, {uid, connectionId});
  };
  if (capabilities.config.backend === 'rust') {
    es.addEventListener('migration-error', event => {
      let reason;
      try { reason = JSON.parse(event.data); }
      catch { reason = {error: '实时同步返回了无效的错误信息，请重试。'}; }
      pauseMigrationWatch(es, uid, agent, reason);
    });
  }
  es.onmessage = ev => {
    // close() 后浏览器仍可能派发已经排队的旧事件，不能让旧 watch 改新视图。
    if (_es !== es || _esUid !== uid || selection.agent !== agent) return;
    let data;
    try { data = JSON.parse(ev.data); }
    catch (error) {
      browserAuditEvent('sse.parse_failed', {error: String(error), bytes: ev.data.length},
        ev.data.slice(0, 4000), {uid, connectionId, severity: 'error'});
      return;
    }
    received++;
    const packetId = data._audit?.packet_id || `${connectionId}:client-${received}`;
    browserAuditEvent('sse.received', {
      packet_id: packetId, kind: data._audit?.kind || '', bytes: ev.data.length,
      reset: !!data.reset, start: data.start, end: data.end,
      messages: data.messages?.length || 0, outbox: data.outbox?.length || 0,
    }, null, {uid, traceId: packetId, connectionId});
    Promise.resolve(applyDiff(uid, data, 0, agent)).then(applied => {
      const snapshot = browserStateSnapshot('sse-applied');
      browserAuditEvent('sse.applied', {
        packet_id: packetId, applied_messages: applied,
        cache_end: cache.get(viewKey(uid, agent))?.end,
      }, snapshot.content, {uid, traceId: packetId, connectionId});
      scheduleBrowserSnapshot('sse-applied');
    }).catch(error => browserAuditEvent('sse.apply_failed', {
      packet_id: packetId, error: String(error?.stack || error),
    }, null, {uid, traceId: packetId, connectionId, severity: 'error'}));
  };
  es.onerror = () => {
    // EventSource 自带的重连会沿用旧 URL(旧偏移), 所以自己关掉重开, 带上新偏移
    es.close();
    browserAuditEvent('sse.error', {ready_state: es.readyState, received, opened}, null,
      {uid, connectionId, severity: 'warning'});
    if (_es !== es) return;
    _es = null;
    clearTimeout(_esRetry);
    if (capabilities.config.backend !== 'rust') {
      _esRetry = setTimeout(() => {
        if (selection.sel === uid && selection.agent === agent) watchSession(uid, agent);
      }, 1500);
      return;
    }
    // Rust：断网、代理断开、服务重启、503 都是瞬时的——按 1.5 s 起的指数退避
    // 重开（封顶 15 s），不暂停视图。流从未打开过时先探一次 HTTP 原因，
    // 只有探到不可恢复的失败（如 501）才暂停。
    const attempt = opened ? 0 : _esRetryAttempt++;
    const delay = retryDelay(attempt);
    const reopen = () => {
      if (selection.sel !== uid || selection.agent !== agent || _es || migrationReadPaused(uid, agent)) return;
      watchSession(uid, agent);
    };
    const schedule = () => { clearTimeout(_esRetry); _esRetry = setTimeout(reopen, delay); };
    if (opened) schedule();
    else probeWatchRejection(uid, agent).then(paused => { if (!paused) schedule(); });
  };
}

function closeWatch() {
  clearTimeout(_esRetry);
  if (_es) {
    browserAuditEvent('sse.closed_by_page', {ready_state: _es.readyState}, null,
      {uid: _esUid, connectionId: _es.__sessiondockConnectionId || ''});
    _es.close(); _es = null; _esUid = null;
  }
}

  return {timing, syncingViews, migrationReadFailures, migrationReadRetries, migrationReadProbes, migrationReadPaused, renderMigrationReadFailure, reportMigrationReadFailure, reportReadFailure, retryMigrationRead, pauseMigrationWatch, probeWatchRejection, syncSession, scheduleDiffRecovery, watchSession, closeWatch, get watching() {return _es;}, get watchedUid() {return _esUid;}};
}
