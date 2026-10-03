// Native identity resolution preserves ambiguity and explicit continuation
// behavior. It never chooses a same-ID copy on another node implicitly.
export function createSessionRouting(catalog, deepNode) {
function uidOfDeepLink(spec) {
  if (!spec) return null;
  const cut = spec.indexOf(':');
  const source = cut > 0 ? spec.slice(0, cut) : null;
  const sid = cut > 0 ? spec.slice(cut + 1) : spec;
  const matches = catalog.sessions.filter(s => s.sid === sid && (!source || s.source === source) && (!deepNode() || s.node_id === deepNode()));
  const byUid = new Map(matches.map(row => [row.uid, row]));
  // Verified rollout generations share one native ID. Collapse only explicit
  // continuations within this identity and machine; independent copies remain ambiguous.
  const current = new Set(matches.map(row => {
    const seen = new Set();
    while (row.continued_in) {
      if (seen.has(row.uid)) return null;
      seen.add(row.uid);
      const next = byUid.get(row.continued_in);
      if (!next || next.source !== row.source || (next.node_id || '') !== (row.node_id || '')) break;
      row = next;
    }
    return row.uid;
  }));
  const hit = (matches.length === 1 ? matches[0]
    : current.size === 1 ? byUid.get(current.values().next().value) : null)
    || catalog.sessions.find(s => s.uid === spec);
  return hit ? hit.uid : null;
}
function agentOfDeepLink(spec) {
  if (!spec) return null;
  const cut=spec.indexOf(':');
  const source=cut>0 ? spec.slice(0,cut) : null;
  const id=cut>0 ? spec.slice(cut+1) : spec;
  const matches=catalog.sessions.filter(s=>(!source || s.source===source) && (!deepNode() || s.node_id===deepNode()))
    .flatMap(s=>(s.agent_items || []).filter(a=>a.id===id).map(a=>({uid:s.uid,agent:a.id})));
  return matches.length===1 ? matches[0] : null;
}
function routeSession(spec) {
  const marker = spec.indexOf('/agent:');
  if (marker >= 0) {
    const uid = uidOfDeepLink(spec.slice(0, marker));
    const id = spec.slice(marker + 7);
    const item = catalog.sessions.find(s => s.uid === uid)?.agent_items?.find(a => a.id === id || a.id === 'agent-' + id);
    return uid && item ? {uid, agent: item.id} : null;
  }
  const uid = uidOfDeepLink(spec);
  return uid ? {uid, agent: null} : agentOfDeepLink(spec);
}
  return {uidOfDeepLink, agentOfDeepLink, routeSession};
}
