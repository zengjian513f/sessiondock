import {viewKey} from '../../domain/runtime/messages.js'
import {transientReadFailure,retryDelay} from '../../domain/runtime/read-failures.js'

// Background unread reads never download whole conversations. Their checkpoint
// ownership is independent from explicit history pagination and live watching.
export function createUnreadService(environment, selection, state, messageCache, reader, status) {
  const {capabilities, fetch, appUrl, MOBILE, HUB_MODE, nodeOf, timing} = environment;
  const {cache} = messageCache;
  const fetchMessages = (...args) => reader.fetchMessages(...args);
  const {unreadRow, addUnread} = status;
function cursorViews(sessions) {
  const rows = [];
  for (const session of sessions) {
    if (session.cursor) rows.push({uid: session.uid, agent: null, cursor: session.cursor});
    for (const item of session.agent_items || []) {
      if (item.cursor) rows.push({uid: session.uid, agent: item.id, cursor: item.cursor});
    }
  }
  return rows;
}

function cleanCursor(value) {
  return value && Number.isFinite(+value.end) && value.head
    ? {end: +value.end, head: String(value.head), anchor: String(value.anchor || '')}
    : null;
}

function seedSidebarCursors(sessions) {
  for (const row of cursorViews(sessions)) {
    const cursor = cleanCursor(row.cursor);
    if (cursor) state.cursors.set(viewKey(row.uid, row.agent), cursor);
  }
}

const sidebarSyncing = new Set(), sidebarPendingCursors = new Map();

// Background views only need unread counts, even when their old history is cached.
// Message bodies are refreshed on selection. Explicit node routes carry local UIDs.
const unreadBatches = new Map(), unreadBatchUnsupported = new Set();
function fetchUnreadSummary(uid, opts) {
  const node = nodeOf(uid);
  if (!capabilities.config.unread_batch || unreadBatchUnsupported.has(node))
    return Promise.resolve({data: {unsupported: true}});
  return new Promise((resolve, reject) => {
    let batch = unreadBatches.get(node);
    if (!batch) {
      batch = [];
      unreadBatches.set(node, batch);
      setTimeout(() => flushUnreadBatch(node, batch), 0);
    }
    batch.push({uid, opts, resolve, reject});
  });
}
async function flushUnreadBatch(node, batch) {
  unreadBatches.delete(node);
  const path = node ? `api/nodes/${node}/api/sessions/unread` : 'api/sessions/unread';
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timing.syncStallMs);
  try {
    const response = await fetch(appUrl(path), {method: 'POST', signal: controller.signal,
      headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({views: batch.map(({uid, opts}) => ({
        uid: node ? uid.replace(`:${node}~`, ':') : uid, agent: opts.agent || '',
        start: opts.start || 0, head: opts.head || '', anchor: opts.anchor || '',
      }))})});
    if ([404, 405, 501].includes(response.status)) {
      await response.arrayBuffer();
      unreadBatchUnsupported.add(node); // Old nodes during a rolling upgrade.
      for (const item of batch) item.resolve({data: {unsupported: true}});
      return;
    }
    const payload = await response.json();
    if (!response.ok || !Array.isArray(payload.results) || payload.results.length !== batch.length) {
      const error = new Error(payload.error || 'Invalid unread summary response');
      error.status = response.ok ? 502 : response.status;
      throw error;
    }
    payload.results.forEach((data, i) => {
      if (data.error) {
        const error = new Error(data.error); error.status = data.status;
        batch[i].reject(error);
      } else batch[i].resolve({data, bytes: 0});
    });
  } catch (error) {
    for (const item of batch) item.reject(error);
  } finally { clearTimeout(timer); }
}

async function syncSidebarView(row, base, latest, attempt = 0) {
  const key = viewKey(row.uid, row.agent);
  if (sidebarSyncing.has(key)) {
    sidebarPendingCursors.set(key, row.agent
      ? {uid: row.uid, agent_items: [{id: row.agent, cursor: latest}]}
      : {uid: row.uid, cursor: latest});
    return;
  }
  sidebarSyncing.add(key);
  try {
    const sameCheckpoint = () => {
      const current = cleanCursor(state.cursors.get(key));
      return current && current.end === base.end && current.head === base.head
        && current.anchor === base.anchor;
    };
    if (!sameCheckpoint()) return;
    const {data} = await fetchUnreadSummary(row.uid, {
      agent: row.agent, start: base.end, head: base.head, anchor: base.anchor,
      appendOnly: true,
    });
    // Selection or another accepted update owns the newer checkpoint. A delayed
    // summary must neither reintroduce unread badges nor rewind that checkpoint.
    if (!sameCheckpoint() || (selection.sel === row.uid && selection.agent === row.agent
        && (!MOBILE.matches || document.body.classList.contains('mobile-detail')))) return;
    if (data.unsupported) {
      // Old nodes cannot give an exact count without downloading history. Show
      // at least one unread item and defer the actual content until selection.
      if (!unreadRow(row.uid).count) addUnread(row.uid, 1);
      state.cursors.set(key, latest);
      return;
    }
    if (data.reset) cache.delete(key);
    else if (data.incoming) addUnread(row.uid, data.incoming);
    state.cursors.set(key, cleanCursor({end: data.end, head: data.version.head,
                                   anchor: data.anchor}) || latest);
  } catch (error) {
    // 列表签名可能不会再变化；瞬时失败（断网、503）按退避补三次（1.5/3/6 s），
    // 仍不影响其他会话；4xx 这类请求本身不成立的失败不重试。
    if (attempt < 3 && (capabilities.config.backend !== 'rust' || transientReadFailure(error))) {
      setTimeout(() => syncSidebarView(row, base, latest, attempt + 1), retryDelay(attempt));
    }
  }
  finally {
    sidebarSyncing.delete(key);
    const pending = sidebarPendingCursors.get(key);
    sidebarPendingCursors.delete(key);
    if (pending) syncSidebarUpdates([pending]);
  }
}

/** 列表发现其他会话文件增长时，只读取追加区间并累加左栏未读数。 */
function syncSidebarUpdates(sessions) {
  for (const row of cursorViews(sessions)) {
    const key = viewKey(row.uid, row.agent);
    const latest = cleanCursor(row.cursor);
    if (!latest) continue;
    const entry = cache.get(key);
    const base = cleanCursor(state.cursors.get(key)) || (entry
      ? cleanCursor({end: entry.end, head: entry.version?.head, anchor: entry.anchor})
      : null);
    if (!base) { state.cursors.set(key, latest); continue; }
    if (!state.cursors.has(key)) state.cursors.set(key, base);
    // Rust 列表行的 cursor 只带物理部分 {end, head}；语义 anchor 只在该会话已被
    // 打开（服务端缓存有视图）时出现。两边都有 anchor 才比较它，缺失时以
    // 已有的 anchor 为准（docs/history-pages.md "列表 cursor"）。
    const anchorSame = !base.anchor || !latest.anchor || base.anchor === latest.anchor;
    if (base.end === latest.end && base.head === latest.head && anchorSame) {
      state.cursors.set(key, latest.anchor ? latest : {...latest, anchor: base.anchor});
      continue;
    }
    const detailVisible = selection.sel === row.uid && selection.agent === row.agent
      && (!MOBILE.matches || document.body.classList.contains('mobile-detail'));
    if (detailVisible) {                     // 当前正在看的新增内容直接视为已读
      state.cursors.set(key, latest);
      continue;
    }
    if (latest.end < base.end) {
      cache.delete(key);                      // 明确回滚，不尝试整份后台下载
      state.cursors.set(key, latest);
      continue;
    }
    void syncSidebarView(row, base, latest);
  }
}

  return {get sidebarSyncing(){return sidebarSyncing},get sidebarPendingCursors(){return sidebarPendingCursors},get unreadBatchUnsupported(){return unreadBatchUnsupported},cursorViews, cleanCursor, seedSidebarCursors, fetchUnreadSummary, flushUnreadBatch, syncSidebarView, syncSidebarUpdates};
}
