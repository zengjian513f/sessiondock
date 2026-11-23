'use strict';

// A worker can send its own receipt while this page cannot run another handler.
(() => {
  if (!SessionDockCapabilities.allows('audit') || !globalThis.Worker) return;
  const script = new URL(document.currentScript.src);
  const asset = new URL('watchdog-worker.js', script);
  asset.search = script.search;
  let worker;
  try { worker = new Worker(asset); } catch { return; }
  const pageId = crypto.randomUUID?.() || Array.from(crypto.getRandomValues(new Uint8Array(16)),
    value => value.toString(16).padStart(2, '0')).join('');
  window.__sessiondockPageId = pageId;
  const query = new URLSearchParams(location.search);
  const surface = document.currentScript.dataset.surface;
  const initial = {uid:query.get('uid') || '', view:surface};
  let readContext = () => initial;
  function context() {
    try { return readContext(); } catch { return initial; }
  }
  let recent = [], frame = 0, suspended = false, active = false;

  function mark(event, details = {}, immediate = false) {
    if (!active) return;
    const point = {event, at: Date.now(), ...details};
    if (immediate) {
      worker.postMessage({type:'points', points:[...recent, point], context:context()});
      recent = [];
    } else {
      recent.push(point);
      if (recent.length > 16) recent.shift();
    }
  }
  globalThis.SessionDockWatchdog = {mark, setContext(read) { readContext = read; }};
  worker.postMessage({type:'init', pageId, surface, build:script.searchParams.get('v') || '',
    hub:SessionDockCapabilities.config.hub === true, node:query.get('node') || ''});
  function lifecycle() {
    active = !suspended && !document.hidden && !globalThis.SessionDockNetwork?.paused;
    cancelAnimationFrame(frame);
    frame = 0;
    recent = [];
    worker.postMessage({type:'active', active, context:context()});
  }
  worker.onmessage = ({data}) => {
    if (!active || data.type !== 'probe') return;
    worker.postMessage({type:'task', epoch:data.epoch, context:context(), points:recent});
    recent = [];
    if (!frame) frame = requestAnimationFrame(() => {
      frame = 0;
      worker.postMessage({type:'frame', epoch:data.epoch});
    });
  };
  for (const type of ['pointerdown', 'click', 'keydown']) {
    addEventListener(type, event => {
      if (!event.isTrusted || event.repeat) return;
      const control = event.target?.closest?.('button,a,input,textarea,select,[role="button"],[data-uid]');
      mark(type, {tag:control?.tagName || '', control:/^[a-z][a-z0-9_-]{0,79}$/i.test(control?.id || '') ? control.id : '',
        delay_ms:Math.max(0, Math.round(performance.now() - event.timeStamp))}, true);
    }, {capture:true, passive:true});
  }
  document.addEventListener('visibilitychange', lifecycle);
  addEventListener('pagehide', () => { suspended = true; lifecycle(); });
  addEventListener('pageshow', () => { suspended = false; lifecycle(); });
  addEventListener('sessiondock-network-paused', lifecycle);
  addEventListener('sessiondock-network-resumed', lifecycle);
  lifecycle();
})();
