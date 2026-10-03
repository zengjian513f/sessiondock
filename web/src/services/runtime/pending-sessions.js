import {pendingUid} from '../../domain/runtime/pending'

export function createPendingSessions(capabilities, SOURCES, terminal, drafts, indexedSessions) {
  const terminalListUncertain = uid => terminal.terminalListUncertain(uid);
const pendingDraftFirstSeen = new Map();
function pendingDraftStartedAt(uid, session) {
  const started = Number(session?.started);
  if (Number.isFinite(started) && started > 0) return started;
  if (!pendingDraftFirstSeen.has(uid)) pendingDraftFirstSeen.set(uid, Date.now() / 1000);
  return pendingDraftFirstSeen.get(uid);
}

/** SessionDock启动、但还没有对话文件的 tmux，也是一条可重新进入的临时会话。 */
function pendingTmuxSessions() {
  if (typeof terminal.state === 'undefined' || !Array.isArray(terminal.state.pending)) return [];
  const pending = [...terminal.state.pending];
  // A CLI may exit before creating native history (for example after updating).
  // Keep its saved input reachable instead of removing the only recovery entry.
  if (typeof drafts !== 'undefined') {
    const names = new Set(pending.map(row => row.name));
    for (const [uid, draft] of drafts) {
      if (!uid.startsWith('tmux:') || !draft.session || names.has(draft.session.name)
          || (!draft.text && !draft.attachments.length && !draft.quotes.length)) continue;
      pending.push({ ...draft.session, stale: true, running: false,
        state: terminalListUncertain(uid) ? 'uncertain' : 'exited',
        started: pendingDraftStartedAt(uid, draft.session), unavailable_reason: '会话草稿已保留' });
    }
  }
  return pending.flatMap(t => {
    // A receipt whose binding the server confirmed is represented by
    // the native row it binds, exactly like a declared Claude identity.
    const native = pendingNativeKey(t);
    if (!SOURCES[t.source] || (native && indexedSessions().byNative.has(native))) return [];
    const source = t.source;
    return [{
      node_id: t.node_id, node_name: t.node_name, stale: t.stale,
      uid: pendingUid(t.name), pending: true, name: t.name, tmuxName: t.name, source,
      ...(capabilities.config.backend === 'rust' ? {record_id:t.record_id,launch_id:t.launch_id,
        instance_id:t.instance_id,running:t.running,state:t.state,unavailable_reason:t.unavailable_reason,
        native_binding:t.native_binding,binding:t.binding,recording:t.recording,grid:t.grid} : {}),
      title: t.title || `新建 ${SOURCES[source].name} 会话`,
      kind: t.kind || '', report_id: t.report_id || '', cwd: t.cwd || '(未知)',
      created: new Date(pendingDraftStartedAt(pendingUid(t.name), t) * 1000).toISOString(),
      updated: new Date(pendingDraftStartedAt(pendingUid(t.name), t) * 1000).toISOString(),
      size: 0,
    }];
  });
}

function pendingNativeKey(t) {
  const declared = t.sid || t.declared_sid || (t.binding?.state === 'confirmed' ? t.binding.sid : '');
  return declared ? JSON.stringify([t.node_id || '', t.source, String(declared)]) : '';
}

// Deleting native rows also discards the launch receipts they hid on the
// server. Drop those receipts here too: otherwise the stale terminal.state.pending brings
// the session back as a pending row until the next terminal list.
function forgetDeletedReceipts(rows) {
  if (typeof terminal.state === 'undefined' || !Array.isArray(terminal.state.pending)) return;
  const keys = new Set(rows.filter(row => row.sid)
    .map(row => JSON.stringify([row.node_id || '', row.source, String(row.sid)])));
  if (keys.size) terminal.state.pending = terminal.state.pending.filter(t => !keys.has(pendingNativeKey(t)));
}

  return {pendingDraftStartedAt, pendingTmuxSessions, pendingNativeKey, forgetDeletedReceipts};
}
