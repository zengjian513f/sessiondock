export function createPostService({fetch, appUrl, buildId: BUILD_ID, pageId: TERM_PAGE_ID, browserAuditEvent, markStaleBuild}) {
async function post(url, body, {timeoutMs = 0} = {}) {
  const traceId = String(body?.request_id || globalThis.crypto?.randomUUID?.()
    || `${Date.now()}-${Math.random().toString(36).slice(2)}`);
  const payload = { ...body, _build: BUILD_ID, _trace_id: traceId,
    _page_id: TERM_PAGE_ID };
  browserAuditEvent?.('http.request.started', {url, method: 'POST'}, null, {
    uid: body?.uid || '', traceId, requestId: body?.request_id || '',
  });
  const started = performance.now();
  let phase = 'headers', headersMs = null, status = null;
  const controller = timeoutMs ? new AbortController() : null;
  const timer = controller ? setTimeout(() => controller.abort(), timeoutMs) : null;
  try {
    const r = await fetch(appUrl(url), {
      method: 'POST', headers: {
        'Content-Type': 'application/json', 'X-SessionDock-Trace': traceId,
        'X-SessionDock-Page': TERM_PAGE_ID, 'X-SessionDock-Build': BUILD_ID,
      },
      body: JSON.stringify(payload),
      ...(controller ? {signal: controller.signal} : {}),
    });
    headersMs = Math.round((performance.now() - started) * 1000) / 1000;
    status = r.status;
    phase = 'body';
    browserAuditEvent?.('http.response.headers', {url, status, headers_ms: headersMs}, null,
      {uid: body?.uid || '', traceId, requestId: body?.request_id || ''});
    const text = await r.text();
    const bodyMs = Math.round((performance.now() - started - headersMs) * 1000) / 1000;
    phase = 'parse';
    let data;
    try {
      data = JSON.parse(text);
    } catch {
      throw new Error(r.status >= 500
        ? `SessionDock 请求失败（HTTP ${r.status}），请稍后重试`
        : 'SessionDock 返回了无法解析的响应，请重新加载');
    }
    browserAuditEvent?.('http.response.received', {
      url, status: r.status, ok: r.ok, headers_ms: headersMs,
      body_ms: bodyMs,
      duration_ms: Math.round((performance.now() - started) * 1000) / 1000,
    }, data, {uid: body?.uid || '', traceId, requestId: body?.request_id || '',
      severity: r.ok ? 'info' : 'warning'});
    if (data?.reload) markStaleBuild(data.build);
    return data;
  } catch (error) {
    if (controller?.signal.aborted) {
      error = new Error('请求超时，服务端可能已执行；请重新核对状态。');
      error.name = 'TimeoutError';
    }
    browserAuditEvent?.('http.request.failed', {
      url, error: String(error?.stack || error), phase, status, headers_ms: headersMs,
      timeout_ms: timeoutMs, online: navigator.onLine, visibility: document.visibilityState,
      duration_ms: Math.round((performance.now() - started) * 1000) / 1000,
    }, null, {uid: body?.uid || '', traceId, requestId: body?.request_id || '',
      severity: 'error'});
    throw error;
  } finally {
    if (timer !== null) clearTimeout(timer);
  }
}

// ---------------------------------------------------------------- 缺陷报告
  return {post};
}
