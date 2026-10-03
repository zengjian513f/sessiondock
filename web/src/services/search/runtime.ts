import { state, status } from '../../stores/search'
import type { SearchBridge, SearchRow, SearchNode } from '../../domain/search/types'
import * as Highlight from './highlight'
import { fetchSearch as requestSearch } from './transport'
let bridge: SearchBridge
const read = () => bridge.read()
const reTerm = Highlight.reTerm
let sequence = 0, runGeneration = 0, searchAbort: AbortController | null = null
let inputTimer: ReturnType<typeof setTimeout> | null = null
export function configure(value: SearchBridge) {
 bridge = value; Highlight.configure(value); renderOpts()
}
export function renderOpts() {state.opts = {...read().opts}}
export function input(value: string) {
 cancelSearch(); state.query = value; bridge.write({term: value.trim()}); state.term = read().term
 inputTimer = setTimeout(() => {inputTimer = null; bridge.renderSide()}, 120)
}
export function keydown(event: KeyboardEvent) {
 if (event.key === 'Escape') {exitSearch(); return}
 if (event.key === 'Enter') void runSearch()
}
export function exitSearch() {cancelSearch(true); bridge.showSessionCount(); bridge.renderSide()}
export function cancelSearch(clearQuery = false) {
 if (inputTimer !== null) clearTimeout(inputTimer)
 inputTimer = null; Highlight.resetRegexSearch(); ++runGeneration
 searchAbort?.abort(); searchAbort = null; searchProgressDone()
 bridge.write({results: null}); bridge.clearFolds()
 if (clearQuery) {bridge.write({term: ''}); state.query = ''}
 state.term = read().term; status.textContent = ''; status.classList.remove('err')
 if (bridge.hub) bridge.clearNodeErrors()
}
export function searchProgress(done: number, total: number | null, nodes: SearchNode[] | null = null, totalKnown = total !== null) {
 state.progress.active = true; state.progress.done = done; state.progress.total = total; state.progress.totalKnown = totalKnown
 if (nodes) state.progress.nodes = nodes
}
export function searchProgressDone() {state.progress.active = false; state.progress.nodes = []}
export function showSearchMatches(rows: SearchRow[]) {
 const found = new Map((read().results || []).map(row => [row.uid, row]))
 for (const row of rows) found.set(row.uid, row)
 bridge.write({results: [...found.values()].sort((a, b) => String(b.updated).localeCompare(String(a.updated)) || b.uid.localeCompare(a.uid))})
 status.textContent = ` 已找到 ${bridge.count(read().results!)} 个会话，继续搜索…`
 bridge.renderSide()
}
export function fetchSearch(params: URLSearchParams, signal: AbortSignal) {
 return requestSearch(params, signal, {fetch: bridge.fetch, allowsSearch: bridge.allowsSearch, appUrl: bridge.appUrl, progress: searchProgress, matches: rows => bridge.showMatches(rows)})
}
export function paintSearchMode(rows: SearchRow[]) {
 state.term = read().term
 state.summary = {active: !!read().term, full: read().results !== null, query: read().term, count: bridge.count(rows)}
 document.querySelector('#side')?.classList.toggle('search-mode', !!read().term)
}
export function toggleOption(key: 'case' | 'word' | 'regex') {
 bridge.write({opts: {...read().opts, [key]: !read().opts[key]}}); optionsChanged()
}
export function toggleMode() {
 if (read().opts.regex) return
 bridge.write({opts: {...read().opts, mode: read().opts.mode === 'any' ? 'all' : 'any'}}); optionsChanged()
}
function optionsChanged() {bridge.persistOptions(read().opts); renderOpts(); if (read().results) void runSearch(); else bridge.renderSide()}
export async function runSearch() {
  state.query = bridge.query();
  const q = state.query.trim();
  cancelSearch();
  bridge.write({term: q});
  const run = runGeneration;
  if (!q) { bridge.write({results: null}); bridge.showSessionCount(); bridge.renderSide(); return; }
  if (!bridge.allowsSearch()) {
    bridge.renderSide();
    status.textContent = ' Rust 后端尚未实现全文搜索；当前只筛选标题和目录。';
    status.classList.add('err');
    status.dataset.seq = ++sequence;
    return;
  }
  if (read().opts.regex && !reTerm(false)) {   // 本地先验一次, 省掉一次全盘扫描
    bridge.write({results: []});
    status.textContent = ' 正则无效';
    status.classList.add('err');
    bridge.renderSide();
    status.dataset.seq = ++sequence;
    return;
  }
  const p = new URLSearchParams({ q });
  if (!read().opts.regex) p.set('mode', read().opts.mode === 'any' ? 'any' : 'all');
  if (bridge.hub) p.set('source', bridge.sources.filter(x => !read().off.has(x)).join(','));
  for (const k of ['case', 'word', 'regex'] as const) if (read().opts[k]) p.set(k, '1');
  const ac = searchAbort = new AbortController();
  bridge.write({results: []});
  status.textContent = ' 正在搜索…';
  bridge.renderSide();
  if (bridge.hub) { bridge.clearNodeErrors(); }
  const searchNodes = bridge.hub ? bridge.nodes().filter(node => bridge.selectedNodeIds().includes(node.id)).map(node => ({
    id: node.id, name: node.name, done: 0, total: node.online === false ? 0 : null,
    state: node.online === false ? 'offline' : 'preparing',
  })) : null;
  searchProgress(0, null, searchNodes, false);
  let response;
  try {
    response = await fetchSearch(p, ac.signal);
  } catch (caught) {
    const e = caught as Error;
    if (e.name === 'AbortError') return;
    response = { ok: false, data: { error: e.message || '搜索失败' } };
  }
  if (run !== runGeneration) return;
  searchAbort = null;
  searchProgressDone();
  const { ok, data: d } = response;
  bridge.applyNodeState(d);
  if (!ok) {                       // 兜底: 前端漏判的非法模式或网络失败
    status.textContent = ` 已找到 ${read().results?.length || 0} 个会话；` + (d.error || '搜索失败');
    status.classList.add('err');
  } else {
    status.classList.remove('err');
    bridge.write({results: d.results!});
    status.textContent = d.truncated
      ? ` 命中超过 ${bridge.count(d.results!)} 个会话（已截断，请细化条件）`
      : ` 全文命中 ${bridge.count(d.results!)} 个会话`;
    if (d.partial) {
      const offline = (d.errors || []).filter(e => d.nodes?.some(n => n.id === e.node_id && n.online === false));
      const failed = (d.errors || []).filter(e => !offline.includes(e));
      if (offline.length) status.textContent += `（${offline.map(e => e.name).join('、')} 离线，未搜索）`;
      if (failed.length) status.textContent += `（${failed.map(e => e.name).join('、')} 搜索失败，结果不完整）`;
    }
  }
  bridge.renderSide();
  // 全文搜索只筛左侧列表；右侧会话的内容、滚动位置和展开状态保持原样。
  status.dataset.seq = ++sequence;              // 供测试判定"这一轮搜索已结束"
}
