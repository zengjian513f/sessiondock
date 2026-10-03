import {expand} from '../../domain/runtime/list-delta.js'
import {useNetworkStore} from '../../stores/runtime/network'

export function createNetworkService(pinia, capabilities, options = {}) {
  const nativeFetch = options.fetch || globalThis.fetch.bind(globalThis);
  const base = new URL('.', location.href);
  const api = new URL('api/', base).pathname;
  const snapshots = new Map();
  const state = useNetworkStore(pinia);
  let sequence = 0;
  const pending = new Set();
  const pausedError = () => new DOMException('自动同步已暂停', 'AbortError');

  function pause(reason) {
    if (reason === 'idle') {
      if (state.sleeping) return;
      state.sleeping = true;
    } else {
      if (state.stopped === 'login' || state.stopped === reason) return;
      state.stopped = reason;
    }
    for (const controller of pending) controller.abort();
    pending.clear();
    dispatchEvent(new CustomEvent('sessiondock-network-paused', {detail: reason}));
  }

  function resume() {
    if (!state.sleeping) return;
    state.sleeping = false;
    // Waking a page must never clear a stale-build or expired-login pause.
    if (!state.stopped) dispatchEvent(new Event('sessiondock-network-resumed'));
  }

  const fetch = async (input, init = {}) => {
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
    if (state.sleeping || (state.stopped && (background || state.stopped === 'login'))) throw pausedError();
    const controller = new AbortController();
    const signal = init.signal || (input instanceof Request ? input.signal : null);
    const abort = () => controller.abort(signal?.reason);
    if (signal?.aborted) abort();
    else signal?.addEventListener('abort', abort, {once:true});
    if (background) pending.add(controller);
    const list = method === 'GET' && capabilities.config.list_delta
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
    try {
      const response = await nativeFetch(list ? url : input, {...init, headers, signal:controller.signal});
      const destination = response.url ? new URL(response.url) : url;
      if (response.status === 401 || (response.redirected && destination.origin === base.origin
          && /\/__auth\//.test(destination.pathname))) {
        pause('login'); throw pausedError();
      }
      if (!list || !response.ok) return response;
      const wire = await response.json();
      if (controller.signal.aborted) throw pausedError();
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
    } finally {
      pending.delete(controller);
      signal?.removeEventListener('abort', abort);
    }
  };
  return {fetch, pause, resume, get paused() {return state.sleeping || !!state.stopped;},
    get reason() {return state.stopped || (state.sleeping ? 'idle' : '');}};
}
