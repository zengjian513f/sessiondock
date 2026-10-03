// Adaptive fallback pacing is shared with SSE acceptance, outside view callbacks.
export function createSyncTicker(network, selection, live, sync) {
  const BACKUP_MS = 20000, SYNC_MS = 10000, FAST_MIN = 350, FAST_MAX = 3000;
function tickSync() {
  if (network.paused) return;
  if (!selection.sel || document.hidden) return;
  if (sync.migrationReadPaused(selection.sel, selection.agent)) return;
  const pushing = sync.watching && sync.watchedUid === selection.sel && sync.watching.readyState === 1;
  const gap = pushing ? BACKUP_MS : (live.live.has(selection.sel) ? selection.syncGap : SYNC_MS);
  if (Date.now() - (selection.lastSync || 0) < gap) return;
  selection.lastSync = Date.now();
  const uid = selection.sel;
  const agent = selection.agent;
  sync.syncSession(uid, agent).then(n => {
    if (uid !== selection.sel || agent !== selection.agent || pushing) return;
    selection.syncGap = n ? FAST_MIN : Math.min(FAST_MAX, Math.round(selection.syncGap * 1.5));
  });
}

  function start() {
    const timer = setInterval(tickSync, 200);
    return () => clearInterval(timer);
  }
  return {tickSync, start};
}
