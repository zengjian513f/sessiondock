import type { Session, Agent, NestRow, RowView, GroupView, Status } from './types'
import { rowKey } from './grouping'
export interface PresentationState {
  view: string; selected: string | null; agent: string | null; picking: boolean;
  live: Set<string>; liveTmux: Set<string>; picked: Set<string>; starBusy: Set<string>; nestAttachUids: string[]; searching: boolean;
}
export interface PresentationHelpers {
  pickable(session: Session): boolean; snippet(session: Session, agent: Agent | null): string;
  highlight(text: string): string; snippetHtml(text: string): string;
  itemMeta(session: Session): string; agentMeta(uid: string, agent: Agent): string;
  agentRunning(uid: string, agent: Agent): boolean; status(session: Session, agent: Agent | null): Status;
  pendingRunning(session: Session): boolean;
  source(source: string): {icon: string; color: string}; path(cwd?: string): string;
  nodeColor(name?: string): string; shortCwd(path: string, length: number): string; nodeDirectory(session: Session): string;
  groupClosed(key: string): boolean; mainMatches(session: Session): boolean;
  groupsAvailable: boolean; groupContains(name: string): boolean;
}
export function rowView(row: NestRow, state: PresentationState, helpers: PresentationHelpers): RowView {
  const s = row.s, agent = row.agent, pickable = !agent && state.picking && helpers.pickable(s)
  const title = agent ? agent.title : s.title, snippet = helpers.snippet(s, agent)
  const path = !agent && state.view === 'date' ? helpers.path(s.cwd) : ''
  const running = agent && helpers.agentRunning(s.uid, agent), source = helpers.source(s.source)
  const pendingRunning = s.pending && helpers.pendingRunning(s)
  const classes = agent ? 'item agent tree' + (running ? ' live' : '') + (state.selected === s.uid && state.agent === agent.id ? ' sel' : '')
    : 'item tree' + (state.selected === s.uid && !state.agent ? ' sel' : '') + (row.closed ? ' nest-closed' : '')
      + (s.pending ? ' pending' + (pendingRunning ? ' live live-tmux' : '') : '')
      + (!s.pending && state.live.has(s.uid) ? ' live' : '') + (!s.pending && state.liveTmux.has(s.uid) ? ' live-tmux' : '')
      + (pickable && state.picked.has(s.uid) ? ' picked' : '') + (state.nestAttachUids.includes(s.uid) ? ' nest-source' : '')
  return {key: rowKey(row), row, classes, title, titleHtml: helpers.highlight(title),
    meta: agent ? helpers.agentMeta(s.uid, agent) : helpers.itemMeta(s), snippet, snippetHtml: snippet ? helpers.snippetHtml(snippet) : '',
    icon: source.icon, iconColor: source.color, starBusy: state.starBusy.has(s.uid), pickable, picked: state.picked.has(s.uid), status: helpers.status(s, agent),
    group: s.group && (!helpers.groupsAvailable || helpers.groupContains(s.group)) ? `分组：${s.group}` : '',
    directory: path ? {path, leaf: path === '/' ? '/' : path.split('/').at(-1)!, node: s.node_name || '', color: helpers.nodeColor(s.node_name)} : undefined}
}
export function groupView(key: string, rows: NestRow[], summary: {first: Session; count: number; pickUids: string[]} | undefined, state: PresentationState, helpers: PresentationHelpers): GroupView {
  const items = rows.filter(row => !row.agent).map(row => row.s), first = summary?.first || items[0] || rows[0]?.s
  const count = summary ? summary.count : state.searching ? rows.filter(row => row.agent || helpers.mainMatches(row.s)).length : items.length
  // Empty named groups never ask for a directory representative.
  const label = state.view === 'tree' ? helpers.nodeDirectory(first!) : state.view === 'group' ? key.slice(6) : key
  const pickUids = summary ? summary.pickUids : items.filter(helpers.pickable).map(s => s.uid)
  const picked = pickUids.filter(uid => state.picked.has(uid)).length
  return {key, label, count, closed: helpers.groupClosed(key), rows: rows.map(row => rowView(row, state, helpers)), pickUids,
    picked: !!pickUids.length && picked === pickUids.length, indeterminate: picked > 0 && picked < pickUids.length,
    node: state.view === 'tree' ? first?.node_name || '' : '', nodeColor: state.view === 'tree' ? helpers.nodeColor(first?.node_name) : '',
    path: state.view === 'tree' ? helpers.shortCwd(first?.cwd || '(未知)', 999) : ''}
}
export interface StatusInput {
  agent?: boolean; running: boolean; count: number; pending: boolean; tmux: boolean; frozen: boolean; attention: string; turn: string; turnLabel: string; attentionLabel: string;
}
export function statusView(input: StatusInput): Status {
  if (input.agent) return {classes: {visible: input.running}, title: input.running ? '子代理运行中' : '', text: ''}
  const {count, running: active, tmux, frozen, attention, turn, pending} = input
  const running = frozen ? '会话已暂停' : (tmux ? '受管会话运行中' : '会话运行中') + input.turnLabel + (attention && turn !== 'waiting' ? input.attentionLabel : '')
  return {frozen, marker: `${frozen}:${count}:${attention}`,
    classes: {frozen, 'input-attention': !frozen && !!attention, 'input-question': !frozen && attention === 'question',
      visible: frozen || active || count > 0, counted: count > 0, tmux, idle: !active,
      'turn-working': !pending && turn === 'working' && !attention, 'turn-waiting': turn === 'waiting'},
    title: count ? `${count} 条新内容，${!active && !frozen ? '会话已退出' : running}` : running,
    text: attention === 'question' && !frozen ? '?' : count > 99 ? '99+' : count || ''}
}
