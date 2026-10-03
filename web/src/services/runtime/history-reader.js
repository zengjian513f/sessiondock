// HTTP history-page streaming retains its own cursor/resume protocol and byte
// accounting. It never writes the live checkpoint or decides a render window.
export function createHistoryReader({fetch, appUrl, pageId: AUDIT_PAGE_ID, buildId: BUILD_ID, browserAuditEvent}) {
async function fetchHistoryPage(uid, agent, cursor, signal, partial) {
  const query = new URLSearchParams({cursor});
  if (agent) query.set('agent', agent);
  if (partial?.resume) {
    query.set('resume', JSON.stringify(partial.resume));
    query.set('next', partial.head);
  }
  const url = `api/messages/${encodeURIComponent(uid)}/page`;
  const traceId = globalThis.crypto?.randomUUID?.() || `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  browserAuditEvent('http.request.started', {url, method: 'GET', agent: agent || '',
    page_start: partial?.head, page_remaining: partial?.omitted, resumable: !!partial?.resume}, null, {uid, traceId});
  let response;
  try {
    response = await fetch(appUrl(`${url}?${query}`), {signal, cache: 'no-store',
      headers: {'X-SessionDock-Trace': traceId, 'X-SessionDock-Page': AUDIT_PAGE_ID,
        'X-SessionDock-Build': BUILD_ID}});
  } catch (error) {
    browserAuditEvent('http.request.failed', {url, error: String(error?.name || error)},
      null, {uid, traceId, severity: 'warning'});
    throw error;
  }
  browserAuditEvent('http.response.received', {url, status: response.status, ok: response.ok},
    null, {uid, traceId, severity: response.ok ? 'info' : 'warning'});
  if (!response.ok) {
    const detail = await response.json().catch(() => null);
    const error = new Error(detail?.error || `HTTP ${response.status}`);
    error.status = response.status;
    throw error;
  }
  const reader = response.body.getReader(), chunks = [];
  let bytes = 0;
  try {
    for (;;) {
      const {done, value} = await reader.read();
      if (done) break;
      bytes += value.length;
      if (bytes > 8 * 1024 * 1024) throw new Error('历史分页响应超过浏览器读取预算。');
      chunks.push(value);
    }
  } catch (error) { await reader.cancel().catch(() => {}); throw error; }
  const buffer = new Uint8Array(bytes);
  let offset = 0;
  for (const chunk of chunks) { buffer.set(chunk, offset); offset += chunk.length; }
  return {data: JSON.parse(new TextDecoder().decode(buffer)), bytes};
}

  return {fetchHistoryPage};
}
