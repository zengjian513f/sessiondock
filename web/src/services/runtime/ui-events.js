import {viewKey} from '../../domain/runtime/messages.js'

export function createUiEvents(environment, catalog, list, terminal, refreshLive, syncSidebarUpdates) {
  const {capabilities, network, appUrl} = environment;
let uiEvents = null, uiEventsReady = false, uiEventsRetry = 0;
let uiEventPending = null, uiEventApplying = false;
function queueUiChange(change) {
  if (!uiEventPending) uiEventPending = {live:false, term:false, sessions:false, cursors:new Map()};
  const pending = uiEventPending;
  for (const key of ['live', 'term', 'sessions']) pending[key] ||= !!(change.initial || change[key]);
  for (const row of change.cursors || []) pending.cursors.set(viewKey(row.uid, row.agent), row);
  void applyUiChanges();
}
async function applyUiChanges() {
  if (uiEventApplying) return;
  uiEventApplying = true;
  try {
    while (uiEventPending && !document.hidden) {
      const change = uiEventPending; uiEventPending = null;
      // An earlier poll may predate this invalidation. Wait for it, then
      // reconcile once more so its old response cannot consume the event.
      if (change.sessions && list.polling) await list.polling;
      const results = await Promise.allSettled([
        change.live ? refreshLive() : null,
        change.term && typeof terminal.loadTermList === 'function'
          ? terminal.loadTermList().then(() => !terminal.state.listError) : null,
        change.sessions ? list.pollSessions() : null,
      ]);
      if (results.some(result => result.status === 'rejected' || result.value === false)) {
        // A delivered event is not an acknowledgement of a successful read.
        // Reconnect obtains a new baseline even if nothing changes afterward.
        closeUiEvents();
        if (!document.hidden) uiEventsRetry = setTimeout(startUiEvents, 3000);
      }
      // A full list read already supplied newer cursors and synchronized
      // unread state. Do not overwrite it with the preceding event snapshot.
      if (change.sessions) continue;
      const rows = [];
      const sessions = new Map(catalog.sessions.map(row => [row.uid, row]));
      for (const update of change.cursors.values()) {
        const session = sessions.get(update.uid);
        if (!session) continue; // A later list invalidation introduces new rows.
        if (update.agent) {
          const agent = session.agent_items?.find(row => row.id === update.agent);
          if (agent) agent.cursor = update.cursor;
          rows.push({uid:update.uid, agent_items:[{id:update.agent,cursor:update.cursor}]});
        } else {
          session.cursor = update.cursor;
          rows.push({uid:update.uid,cursor:update.cursor});
        }
      }
      if (rows.length) syncSidebarUpdates(rows);
    }
  } finally { uiEventApplying = false; }
}
function closeUiEvents() {
  clearTimeout(uiEventsRetry);
  uiEvents?.close(); uiEvents = null; uiEventsReady = false;
}
function startUiEvents() {
  if (network.paused) return;
  if (!capabilities.config.ui_events || !window.EventSource || document.hidden || uiEvents) return;
  clearTimeout(uiEventsRetry);
  const stream = new EventSource(appUrl('api/events'));
  uiEvents = stream;
  stream.addEventListener('change', event => {
    if (uiEvents !== stream) return;
    try {
      const change = JSON.parse(event.data);
      uiEventsReady = !change.retry;
      if (!change.retry) queueUiChange(change);
    } catch {
      closeUiEvents();
      if (!document.hidden) uiEventsRetry = setTimeout(startUiEvents, 3000);
    }
  });
  stream.onerror = () => {
    if (uiEvents !== stream) return;
    closeUiEvents();
    // Polling is a disconnected/old-server fallback, never parallel upkeep
    // of a healthy push connection. Reconnect sends a fresh baseline.
    if (!document.hidden) uiEventsRetry = setTimeout(startUiEvents, 3000);
  };
}
  function start() {
    document.addEventListener('visibilitychange', () => {
      if (document.hidden) closeUiEvents(); else startUiEvents();
    });
    addEventListener('pagehide', closeUiEvents);
    addEventListener('pageshow', startUiEvents);
    setTimeout(startUiEvents, 0);
  }
  return {queueUiChange, applyUiChanges, closeUiEvents, startUiEvents, start,
    get connection(){return uiEvents},get retry(){return uiEventsRetry},get applying(){return uiEventApplying},get ready() {return uiEventsReady;}};
}
