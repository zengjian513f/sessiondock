'use strict';

// Transport snapshots are separate from UI rows: rendering annotates/mutates
// those rows, while a delta must always apply to its original wire baseline.
globalThis.SessionDockNetwork = (() => {
  const nativeFetch = globalThis.fetch.bind(globalThis);
  const base = new URL('.', location.href);
  const api = new URL('api/', base).pathname;
  const snapshots = new Map();
  let stopped = '', sleeping = false, sequence = 0;
  let backgroundAbort = new AbortController();
  const pausedError = () => new DOMException('自动同步已暂停', 'AbortError');
  const rowKey = (row, key) => `${row.node_id || ''}\n${row[key]}`;

  function pause(reason) {
    if (reason === 'idle') {
      if (sleeping) return;
      sleeping = true;
    } else {
      if (stopped === 'login' || stopped === reason) return;
      stopped = reason;
    }
    backgroundAbort.abort(pausedError());
    dispatchEvent(new CustomEvent('sessiondock-network-paused', {detail: reason}));
  }

  function resume() {
    if (!sleeping) return;
    sleeping = false;
    backgroundAbort = new AbortController();
    // Waking a page must never clear a stale-build or expired-login pause.
    if (!stopped) dispatchEvent(new Event('sessiondock-network-resumed'));
  }

  function expandRows(baseline, patch) {
    const old = new Map(baseline.map(row => [rowKey(row, patch.key), row]));
    const removed = new Set(patch.remove);
    const replaced = new Set(patch.upsert.map(item => item.id ?? rowKey(item.row, patch.key)));
    let rows = baseline.filter(row => !removed.has(rowKey(row, patch.key)) && !replaced.has(rowKey(row, patch.key)));
    for (const item of patch.upsert) {
      if (!Number.isInteger(item.index) || item.index < 0 || item.index > rows.length) throw new Error('列表增量顺序无效');
      let row = item.row;
      if (item.id !== undefined) {
        if (!old.has(item.id)) throw new Error('列表增量缺少条目');
        row = {...old.get(item.id), ...item.set};
        for (const field of item.unset) delete row[field];
        if (item.agents) row.agent_items = expandRows(row.agent_items, item.agents);
      }
      rows.splice(item.index, 0, row);
    }
    if (patch.order) {
      const byId = new Map(rows.map(row => [rowKey(row, patch.key), row]));
      rows = patch.order.map(id => {
        if (!byId.has(id)) throw new Error('列表增量缺少条目');
        const row = byId.get(id); byId.delete(id); return row;
      });
      if (byId.size) throw new Error('列表增量顺序不完整');
    }
    return rows;
  }

  function expand(wire, baseline) {
    const delta = wire.list_delta;
    if (!delta) return wire;
    if (!baseline || delta.base !== baseline.list_version) throw new Error('列表同步基线已失效');
    const result = {...wire};
    for (const [name, patch] of Object.entries(delta.collections)) result[name] = expandRows(baseline[name], patch);
    delete result.list_delta;
    delete result.list_unchanged;
    return result;
  }

  globalThis.fetch = async (input, init = {}) => {
    const url = new URL(input instanceof Request ? input.url : input, location.href);
    const internal = url.origin === base.origin && url.pathname.startsWith(api);
    if (!internal) return nativeFetch(input, init);
    const method = (init.method || (input instanceof Request ? input.method : 'GET')).toUpperCase();
    // Draft hydration/conflict reads belong to saving an editor. Automatic
    // draft-follow is paused by its caller, so saves can still rebase safely.
    const draftRead = method === 'GET' && url.pathname === api + 'session/conversation';
    const background = (method === 'GET' && !draftRead) || /\/api\/(audit\/browser|sessions\/unread|session\/conversation\/check)$/.test(url.pathname);
    // A stale page may still save editor drafts before reloading. An expired
    // login pauses writes too: repeatedly following the login redirect is waste.
    if (sleeping || (stopped && (background || stopped === 'login'))) throw pausedError();
    const signal = init.signal || (input instanceof Request ? input.signal : null);
    // Fetch resolves at the headers, while its body may still be streaming.
    // Native signal composition keeps cancellation alive for that whole body,
    // without retaining a listener for every request on the page's pause signal.
    const signals = [signal, background ? backgroundAbort.signal : null].filter(Boolean);
    const requestSignal = signals.length ? AbortSignal.any(signals) : undefined;
    const list = method === 'GET' && SessionDockCapabilities.config.list_delta
      && [api + 'sessions', api + 'term/list'].includes(url.pathname);
    const headers = new Headers(input instanceof Request ? input.headers : undefined);
    new Headers(init.headers).forEach((v, k) => headers.set(k, v));
    let cacheKey, baseline, run;
    if (list) {
      url.searchParams.delete('sig');
      const key = new URL(url); key.searchParams.delete('force'); key.searchParams.sort();
      cacheKey = key.toString(); baseline = snapshots.get(cacheKey)?.data; run = ++sequence;
      headers.set('X-SessionDock-List', baseline?.list_version || 'new');
    }
    const response = await nativeFetch(list ? url : input, {...init, headers, signal:requestSignal});
    const destination = response.url ? new URL(response.url) : url;
    if (response.status === 401 || (response.redirected && destination.origin === base.origin
        && /\/__auth\//.test(destination.pathname))) {
      pause('login'); throw pausedError();
    }
    if (!list || !response.ok) return response;
    const wire = await response.json();
    requestSignal?.throwIfAborted();
    const data = expand(wire, baseline);
    if (data.list_version && (!snapshots.has(cacheKey) || snapshots.get(cacheKey).run < run)) {
      snapshots.set(cacheKey, {run, data});
    }
    // Return a fresh decoded object to the page; the transport baseline above
    // never becomes S.sessions/T.list and cannot acquire UI-only mutations.
    const output = {...data};
    if (wire.list_delta && wire.list_unchanged && url.pathname === api + 'sessions') output.unchanged = true;
    const resultHeaders = new Headers(response.headers);
    resultHeaders.delete('Content-Length'); resultHeaders.delete('Content-Encoding');
    return new Response(JSON.stringify(output), {status:response.status, statusText:response.statusText, headers:resultHeaders});
  };
  return Object.freeze({pause, resume, get paused() {return sleeping || !!stopped;},
    get reason() {return stopped || (sleeping ? 'idle' : '');}});
})();
