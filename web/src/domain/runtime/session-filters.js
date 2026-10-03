// Selection, quick/full search and tree ancestry remain one consistent filter.
export function createSessionFilters({selection, search, live, sidebar}, index, nodes, groups, searchRows, nestEdges) {
  const {sessionHidden, sidebarSessions, indexedSessions} = index;
  const {nodeSelected} = nodes;
  const {sidebarMainMatches, sidebarAgentItems} = searchRows;
function visible() {
  const eligible = s => (!sessionHidden(s) || s.uid === selection.sel) && !sidebar.off.has(s.source)
    && nodeSelected(s) && groups.matches(s);
  let pool = (search.results || sidebarSessions()).filter(eligible);
  if (sidebar.activeOnly) pool = pool.filter(s => s.pending || live.live.has(s.uid));
  if (search.term && search.results === null) pool = pool.filter(s => sidebarMainMatches(s)
    || sidebarAgentItems(s).length > 0);
  if (search.term && sidebar.nest && sidebar.view !== 'group') {
    // Retain only the ancestors needed to place matched rows in the tree.
    // They carry structural metadata, never a child's snippet or siblings.
    const found = new Map(pool.map(s => [s.uid, s]));
    const parents = new Map();
    const {children} = nestEdges(sidebarSessions().filter(eligible));
    for (const [uid, rows] of children) for (const row of rows) parents.set(row.uid, uid);
    const indexed = indexedSessions().byUid;
    for (const match of pool) {
      const seen = new Set([match.uid]);
      let uid = parents.get(match.uid);
      while (uid && !seen.has(uid)) {
        seen.add(uid);
        if (!found.has(uid)) {
          const parent = indexed.get(uid);
          if (parent) found.set(uid, {...parent, hits: 0, hits_capped: false, snippet: '', agent_items: []});
        }
        uid = parents.get(uid);
      }
    }
    pool = [...found.values()];
  }
  return pool;
}

  return {visible};
}
