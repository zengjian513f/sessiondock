const $ = selector => document.querySelector(selector);

import * as Conversation from '../../migration/conversation'

import {viewKey,entryTotal} from '../../domain/runtime/messages.js'

export function createCatalogMeta({core,conversationRenderer,sessionUi}) {
function mergeSessionMetaEvent(entry, session) {
  if (entry.meta.agent_id || !session.renamed_at || !session.renamed_to) return false;
  const eventId = `rename:${session.sid}:${session.renamed_at}`;
  if (entry.msgs.some(m => m.event_id === eventId)) return false;
  const event = {
    role: 'command', text: `/rename ${session.renamed_to}`, ts: session.renamed_at,
    counted: false, inferred: true, event_id: eventId,
  };
  const at = entry.msgs.findIndex(m => m.ts && m.ts > event.ts);
  entry.msgs.splice(at < 0 ? entry.msgs.length : at, 0, event);
  Conversation.index.messageIndexes.delete(entry.msgs);
  return true;
}

function refreshSessionMeta() {
  const headerKey = m => JSON.stringify([
    m.title, m.parent_title, m.sid, m.agent_type, m.model, !!m.starred,
    m.nest_parent || null, m.group || null,
    (m.agent_items || []).map(a => [a.id, a.title, a.type]),
  ]);
  const before = core.cache.cache.get(viewKey(core.state.selection.sel, core.state.selection.agent));
  const beforeKey = before ? headerKey(before.meta) : '';
  let currentEventAdded = false;
  for (const e of core.cache.cache.values()) {
    const s = core.index.indexedSessions().byUid.get(e.meta.uid);
    if (s) {
      const agent = e.meta.agent_id;
      if (!agent) {
        e.meta = { ...e.meta, ...s };
        if (mergeSessionMetaEvent(e, s) && e === before) currentEventAdded = true;
        continue;
      }
      const item = (s.agent_items || []).find(a => a.id === agent);
      if (!item) continue;
      const childPath = e.meta.path;
      e.meta = {
        ...e.meta, ...s, path: childPath,
        cwd: item.cwd ?? e.meta.cwd, model: item.model ?? e.meta.model,
        created: item.created ?? e.meta.created,
        sid: agent, title: item.title, size: item.size, updated: item.updated,
        agent_id: agent, agent_type: item.type, parent_title: s.title,
      };
    }
  }
  const current = core.cache.cache.get(viewKey(core.state.selection.sel, core.state.selection.agent));
  const oldHead = $('#detail > .dhead');
  if (currentEventAdded && current) {
    conversationRenderer().renderSession(current.meta, current.msgs, current.activity);
  } else if (current && oldHead && headerKey(current.meta) !== beforeKey) {
    oldHead.replaceWith(sessionUi().head(current.meta, entryTotal(current)));
    sessionUi().layoutSessionHead();
    core.diagnostics.auditDetailRendered('meta-refresh');
  }
}
return {mergeSessionMetaEvent,refreshSessionMeta};
}
