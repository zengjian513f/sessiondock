import type { Session, Agent, NestRow } from './types'
export interface GroupingContext {
  view: string; nest: boolean; term: string; picking: boolean; live: Set<string>; sessions: Session[];
  groupClosed(key: string): boolean; nestClosed(uid: string): boolean;
  mainMatches(session: Session): boolean; agents(session: Session): Agent[];
  agentRunning(uid: string, agent: Agent): boolean; pickable(session: Session): boolean;
  dayKey(stamp: number): string; names: string[]; available: boolean; contains(name: string): boolean;
  continued(session: Session): boolean; byUid(uid: string): Session | undefined;
  expand(s: Session, depth: number, children: Children, out: NestRow[], seen: Set<string>, memo: Map<string, number>, sizes: Map<string, Size>): void;
  hiddenForkParent(session: Session): boolean; forkChildren(session: Session): Session[];
}
export type Children = Map<string, Session[]>
export interface Size { rows: number; sessions: number }
export interface ClosedSummary {first: Session; count: number; pickUids: string[]; roots: Session[]}
export type Group = [string, NestRow[], ClosedSummary?]
export type Groups = Group[] & {children: Children}
const stamp = (value: string) => +new Date(value) || 0
const spawnKey = (node: string | undefined, source: string, sid: string) => JSON.stringify([node || '', source, String(sid)])
export function parentOf(session: Session, byKey: Map<string, Session>, all: Map<string, Session>, ctx: GroupingContext): Session | null {
  const spec = session.nest_parent
  if (!spec?.source || !spec.sid) return null
  let row = byKey.get(spawnKey(spec.node_id || session.node_id, spec.source, spec.sid)) || all.get(spawnKey(spec.node_id || session.node_id, spec.source, spec.sid))
  const seen = new Set([session.uid])
  while (row && !seen.has(row.uid)) {
    seen.add(row.uid)
    const visible = byKey.get(spawnKey(row.node_id, row.source, row.sid))
    if (visible) return visible
    if (ctx.continued(row)) {
      const next = ctx.byUid(row.continued_in)
      row = next?.source === row.source && (next.node_id || '') === (row.node_id || '') ? next : undefined
    } else if (ctx.hiddenForkParent(row)) row = ctx.forkChildren(row)[0]
    else return null
  }
  return null
}
export function nestEdges(list: Session[], ctx: GroupingContext): {children: Children; nested: Set<string>} {
  const children: Children = new Map(), nested = new Set<string>()
  const byKey = new Map(list.map(s => [spawnKey(s.node_id, s.source, s.sid), s]))
  const all = new Map(ctx.sessions.map(s => [spawnKey(s.node_id, s.source, s.sid), s]))
  const parents = new Map<string, Session>()
  for (const s of list) {
    const parent = parentOf(s, byKey, all, ctx)
    if (!parent) continue
    let cur: Session | undefined = parent, looped = false
    for (let i = 0; cur && i < list.length; i++) {
      if (cur === s) { looped = true; break }
      cur = parents.get(cur.uid)
    }
    if (looped) continue
    parents.set(s.uid, parent)
    if (!children.has(parent.uid)) children.set(parent.uid, [])
    children.get(parent.uid)!.push(s); nested.add(s.uid)
  }
  return {children, nested}
}
export function nestStamp(s: Session, children: Children, memo = new Map<string, number>()): number {
  if (memo.has(s.uid)) return memo.get(s.uid)!
  let latest = stamp(s.updated); memo.set(s.uid, latest)
  for (const a of s.agent_items || []) latest = Math.max(latest, stamp(a.updated))
  for (const c of children.get(s.uid) || []) latest = Math.max(latest, nestStamp(c, children, memo))
  memo.set(s.uid, latest); return latest
}
export function nestSize(s: Session, children: Children, ctx: GroupingContext, memo = new Map<string, Size>()): Size {
  if (memo.has(s.uid)) return memo.get(s.uid)!
  const size = {rows: 1 + ctx.agents(s).length, sessions: 1}; memo.set(s.uid, size)
  for (const child of children.get(s.uid) || []) {
    const sub = ctx.nestClosed(child.uid) ? {rows: 1, sessions: 1} : nestSize(child, children, ctx, memo)
    size.rows += sub.rows; size.sessions += sub.sessions
  }
  return size
}
export function expandRows(s: Session, depth: number, children: Children, out: NestRow[], seen: Set<string>, ctx: GroupingContext, memo = new Map<string, number>(), sizes = new Map<string, Size>()): void {
  const row: NestRow = {s, agent: null, depth, kids: 0, closed: false}
  const showMain = ctx.mainMatches(s) || (ctx.nest && ctx.view !== 'group')
  if (showMain) out.push(row)
  if (ctx.nestClosed(s.uid)) { row.kids = nestSize(s, children, ctx, sizes).rows - 1; row.closed = row.kids > 0; return }
  const kids: {agent?: Agent; session?: Session; running: boolean; when: number}[] = [
    ...ctx.agents(s).map(agent => ({agent, running: ctx.agentRunning(s.uid, agent), when: stamp(agent.updated)})),
    ...(children.get(s.uid) || []).map(session => ({session, running: ctx.live.has(session.uid), when: nestStamp(session, children, memo)})),
  ].sort((a, b) => Number(b.running) - Number(a.running) || b.when - a.when)
  row.closed = kids.length > 0 && ctx.nestClosed(s.uid)
  const start = out.length
  for (const k of kids) {
    if (k.agent) out.push({s, agent: k.agent, depth: depth + Number(showMain)})
    else if (k.session && !seen.has(k.session.uid)) { seen.add(k.session.uid); ctx.expand(k.session, depth + Number(showMain), children, out, seen, memo, sizes) }
  }
  row.kids = out.length - start
}
export function groupBy(list: Session[], ctx: GroupingContext, skipClosed = false): Groups {
  const {children, nested} = ctx.view === 'group' || !ctx.nest ? {children: new Map<string, Session[]>(), nested: new Set<string>()} : nestEdges(list, ctx)
  const memo = new Map<string, number>(), sizes = new Map<string, Size>()
  const time = (s: Session) => ctx.nest ? nestStamp(s, children, memo) : stamp(s.updated)
  const groups = new Map<string, Session[]>(), latest = new Map<string, number>(), dates = new Map<number, string>()
  for (const s of list) {
    if (nested.has(s.uid) || (ctx.view === 'group' && (!s.group || (ctx.available && !ctx.contains(s.group))))) continue
    const updated = time(s)
    if (ctx.view === 'date' && !dates.has(updated)) dates.set(updated, ctx.dayKey(updated))
    const key = ctx.view === 'tree' ? (s.node_id ? JSON.stringify([s.node_id, s.cwd || '(未知)']) : s.cwd || '(未知)') : ctx.view === 'group' ? `group:${s.group || ''}` : dates.get(updated)!
    if (!groups.has(key)) groups.set(key, [])
    groups.get(key)!.push(s); latest.set(key, Math.max(latest.get(key) ?? -Infinity, updated))
  }
  if (ctx.view === 'group' && !ctx.term) for (const name of ctx.names) {
    const key = `group:${name}`; if (!groups.has(key)) {groups.set(key, []); latest.set(key, -Infinity)}
  }
  const keys = [...groups.keys()]
  if (ctx.view === 'date') keys.sort().reverse(); else keys.sort((a, b) => latest.get(b)! - latest.get(a)!)
  const compare = (a: Session, b: Session) => (ctx.view === 'date' ? Number(!!b.starred) - Number(!!a.starred) : 0) || time(b) - time(a)
  const seen = new Set<string>()
  const result = keys.map((key): Group => {
    const roots = groups.get(key)!
    if (!roots.length) return [key, []]
    if (skipClosed && ctx.groupClosed(key)) {
      let first = roots[0]!, count = 0
      for (const s of roots) {if (compare(s, first) < 0) first = s; count += !ctx.nest || ctx.nestClosed(s.uid) ? 1 : nestSize(s, children, ctx, sizes).sessions}
      const pickUids: string[] = []
      if (ctx.picking) {
        const collect = (s: Session) => {if (ctx.pickable(s)) pickUids.push(s.uid); if (ctx.nest && !ctx.nestClosed(s.uid)) for (const child of children.get(s.uid) || []) collect(child)}
        roots.forEach(collect)
      }
      return [key, [], {first, count, pickUids, roots}]
    }
    roots.sort(compare)
    const rows: NestRow[] = []
    for (const s of roots) {if (seen.has(s.uid)) continue; seen.add(s.uid); ctx.expand(s, 0, children, rows, seen, memo, sizes)}
    return [key, rows]
  }) as Groups
  result.children = children; return result
}
export const rowKey = (row: NestRow) => row.agent ? `${row.s.uid}#${row.agent.id}` : row.s.uid
