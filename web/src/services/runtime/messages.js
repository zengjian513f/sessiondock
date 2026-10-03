// Download/read ownership stays outside views, including streaming byte progress.
export function createMessageReader({fetch, appUrl, capabilities, pageId: AUDIT_PAGE_ID, buildId: BUILD_ID, browserAuditEvent}) {
async function fetchMessages(uid, opts = {}) {
  const p = new URLSearchParams();
  if (opts.start) {
    p.set('start', opts.start);
    p.set('head', opts.head);
    p.set('anchor', opts.anchor || '');     // 没有锚点服务端会拒绝续读, 直接给整份
  }
  if (opts.agent) p.set('agent', opts.agent);
  if (opts.appendOnly) p.set('append', '1');
  if (opts.windowed) p.set('window', '1');
  const traceId = globalThis.crypto?.randomUUID?.()
    || `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  const url = `api/messages/${encodeURIComponent(uid)}?${p}`;
  const started = performance.now();
  browserAuditEvent('http.request.started', {
    url, method: 'GET', start: opts.start || 0, agent: opts.agent || '',
  }, null, {uid, traceId});
  let r;
  try {
    r = await fetch(appUrl(url), {
      signal: opts.signal,
      headers: {'X-SessionDock-Trace': traceId, 'X-SessionDock-Page': AUDIT_PAGE_ID,
        'X-SessionDock-Build': BUILD_ID},
    });
  } catch (error) {
    browserAuditEvent('http.request.failed', {
      url, error: String(error?.name || error),
      duration_ms: Math.round((performance.now() - started) * 1000) / 1000,
    }, null, {uid, traceId, severity: error?.name === 'AbortError' ? 'warning' : 'error'});
    throw error;
  }
  opts.onActivity?.();
  if (!r.ok) {
    browserAuditEvent('http.response.received', {url, status: r.status, ok: false},
      null, {uid, traceId, severity: 'warning'});
    const detail = capabilities.config.backend === 'rust'
      ? await r.json().catch(() => null) : null;
    const error = new Error(detail?.error || 'HTTP ' + r.status);
    error.status = r.status;
    error.code = detail?.code || '';
    throw error;
  }
  // Fetch 自动解压 gzip：reader.read() 统计的是解压后字节，而标准
  // Content-Length 仍可能是压缩后大小。优先用服务端给出的同口径长度；
  // 连到旧服务端时，压缩响应改显示不定进度，也不伪造一个较小的分母。
  const contentTotal = +r.headers.get('Content-Length') || 0;
  const decodedTotal = +r.headers.get('X-SessionDock-Decoded-Length') || 0;
  const encoded = !!r.headers.get('Content-Encoding');
  const total = decodedTotal || (encoded ? 0 : contentTotal);
  const reader = r.body.getReader();
  const chunks = [];
  let got = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    got += value.length;
    opts.onActivity?.();
    if (encoded && contentTotal && decodedTotal) {
      // Fetch 不暴露实时压缩字节数。用解压进度映射到已知的
      // 压缩总量：中途值明确标为估算，最后一帧则精确等于响应体流量。
      const transferred = Math.min(contentTotal,
        Math.round(got / decodedTotal * contentTotal));
      opts.onProgress?.(transferred, contentTotal, {
        compressed: true, estimated: got < decodedTotal,
      });
    } else {
      opts.onProgress?.(got, total);
    }
  }
  const buf = new Uint8Array(got);
  let at = 0;
  for (const c of chunks) { buf.set(c, at); at += c.length; }
  const data = JSON.parse(new TextDecoder().decode(buf));
  browserAuditEvent('http.response.parsed', {
    url, status: r.status, bytes: got, reset: !!data.reset,
    start: data.start, end: data.end, messages: data.messages?.length || 0,
    duration_ms: Math.round((performance.now() - started) * 1000) / 1000,
  }, null, {uid, traceId});
  return { data, bytes: got,
    networkBytes: encoded && contentTotal ? contentTotal : got };
}

  return {fetchMessages};
}
