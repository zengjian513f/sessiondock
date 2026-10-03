// Indexes are derived from the identity of the raw catalog array.
export function createSessionIndex({catalog, live, sidebar}, pendingTmuxSessions) {
let sessionIndexRows = null, sessionIndex = null;
function indexedSessions() {
  if (sessionIndexRows !== catalog.sessions) {
    sessionIndexRows = catalog.sessions;
    const byUid = new Map(), byNative = new Map(), forkChildren = new Map();
    for (const row of catalog.sessions) {
      byUid.set(row.uid, row);
      const key = JSON.stringify([row.node_id || '', row.source, String(row.sid)]);
      if (!byNative.has(key)) byNative.set(key, row);
      if (row.sid && row.forked_from_id) {
        const parent = JSON.stringify([row.node_id || '', row.source, String(row.forked_from_id)]);
        if (!forkChildren.has(parent)) forkChildren.set(parent, []);
        forkChildren.get(parent).push(row);
      }
    }
    sessionIndex = {byUid, byNative, forkChildren};
  }
  return sessionIndex;
}
const sessionContinued = session =>
  !!(session?.continued_in && indexedSessions().byUid.has(session.continued_in));
const hiddenForkParent = session => !!session?.fork_parent && !session.fork_parent_visible;
const sessionHidden = session => hiddenForkParent(session) || sessionContinued(session);
// 沿 forked_from_id 往上追整条父会话链（近的在前）。只在同来源、同机器内按
// 原生 sid 匹配；记录已不存在的一级保留占位并到此为止。
function forkAncestors(session) {
  const chain = [];
  const seen = new Set();
  let sid = String(session?.forked_from_id || '');
  while (sid && !seen.has(sid)) {
    seen.add(sid);
    const row = indexedSessions().byNative.get(JSON.stringify([session.node_id || '', session.source, sid]));
    chain.push({ sid, row: row || null });
    sid = String(row?.forked_from_id || '');
  }
  return chain;
}
// 沿 forked_from_id 往下追到最深的回退分支：Codex 双 Esc 后进程不变，新消息只写
// 进新分支的文件。同一级有多条分支时先取仍在运行的，再取最新创建的。
// 一条会话的直接子分支（同来源、同机器，forked_from_id 指向它）：仍在运行的
// 在前，其余按创建时间新的在前。
function forkChildren(session) {
  if (!session?.sid) return [];
  return [...(indexedSessions().forkChildren.get(JSON.stringify([session.node_id || '', session.source, String(session.sid)])) || [])]
    .sort((a, b) => (live.live.has(b.uid) - live.live.has(a.uid))
      || String(b.created || '').localeCompare(String(a.created || '')));
}
function forkLeaf(session) {
  const seen = new Set();
  let cur = session;
  while (cur && !seen.has(cur.uid)) {
    seen.add(cur.uid);
    const children = forkChildren(cur);
    if (!children.length) break;
    cur = children[0];
  }
  return cur;
}
function forkLeafUid(uid) {
  return forkLeaf(catalog.sessions.find(s => s.uid === uid))?.uid || uid;
}
const sidebarSessions = () => [...pendingTmuxSessions(), ...catalog.sessions]
  .filter(s => !sessionHidden(s));

  return {indexedSessions, forkAncestors, forkChildren, forkLeaf, forkLeafUid, sessionContinued, hiddenForkParent, sessionHidden, sidebarSessions};
}
