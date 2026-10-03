import * as Query from '../../domain/search/query'
import type { SearchBridge, RegexRanges } from '../../domain/search/types'
import { state } from '../../stores/search'
export const MARK_MAX = 3000, AUTO_OPEN_MAX = 40
const SEARCH_ROLES = new Set(['user', 'assistant', 'user·subagent', 'assistant·subagent', 'thinking', 'question', 'answer', 'command'])
interface MatchElement extends HTMLElement {_applyRegexMatch?: () => void}
let bridge: SearchBridge
export function configure(value: SearchBridge) {bridge = value}
const read = () => bridge.read()
const esc = (text: string) => bridge.escape(text)
const $ = (selector: string) => document.querySelector<HTMLElement>(selector)
export function reTerm(global = false) {return Query.reTerm(read().term, read().opts, global)}
export const searchTerms = Query.searchTerms
export function literalSource(term: string) {return Query.literalSource(term, read().opts)}
let regexWorker: Worker | null = null, regexGeneration = '', regexRequest = 0, regexPaintTimer: ReturnType<typeof setTimeout> | null = null;

const regexResults = new Map<string, RegexRanges>(), regexPending = new Map<number, {text: string; resolve: (ranges: RegexRanges) => void}>(), regexByText = new Map<string, Promise<RegexRanges>>();
export function resetRegexSearch() {
  regexWorker?.terminate();
  regexWorker = null;
  regexGeneration = '';
  regexResults.clear();
  for (const job of regexPending.values()) job.resolve([]);
  regexPending.clear();
  regexByText.clear();
  if (regexPaintTimer !== null) clearTimeout(regexPaintTimer);
  regexPaintTimer = null;
}

export function regexMatches(text: unknown): Promise<RegexRanges> {
  const re = reTerm(true);
  if (!re) return Promise.resolve([]);
  const generation = `${re.source}\0${re.flags}`;
  if (regexGeneration !== generation) {
    resetRegexSearch();
    regexGeneration = generation;
  }
  const value = String(text ?? '');
  if (regexResults.has(value)) return Promise.resolve(regexResults.get(value)!);
  if (regexByText.has(value)) return regexByText.get(value)!;
  if (!regexWorker) {
    regexWorker = new Worker(bridge.appUrl('regex-worker.js'));
    regexWorker.onmessage = ({data}: MessageEvent<{id: number; ranges: RegexRanges; matched?: boolean}>) => {
      const job = regexPending.get(data.id);
      if (!job) return;
      regexPending.delete(data.id);
      regexByText.delete(job.text);
      data.ranges.matched = !!data.matched;
      regexResults.set(job.text, data.ranges);
      job.resolve(data.ranges);
      if (regexPaintTimer === null) regexPaintTimer = setTimeout(() => {
        regexPaintTimer = null;
        state.revision++;
        bridge.renderSide();
      }, 50);
    };
    regexWorker.onerror = () => resetRegexSearch();
  }
  const id = ++regexRequest;
  let resolve!: (ranges: RegexRanges) => void;
  const promise = new Promise<RegexRanges>(done => { resolve = done; });
  regexPending.set(id, {text: value, resolve});
  regexByText.set(value, promise);
  regexWorker.postMessage({id, text: value, source: re.source, flags: re.flags, limit: MARK_MAX});
  return promise;
}

export function regexCached(text: unknown): RegexRanges {
  if (!reTerm(true)) return [];
  // Starting a request also invalidates results from a changed query/options.
  regexMatches(text);
  return regexResults.get(String(text ?? '')) || [];
}

export function matchesSearch(text: string) {return Query.matchesSearch(text, read().term, read().opts, hasTerm)}

export function hasTerm(t: string) {
  if (!read().term) return false;
  if (read().opts.regex) return !!regexCached(t).matched;
  const re = reTerm(false);
  return re ? re.test(t) : false;
}

export function hl(text: unknown) {
  const value = String(text);
  if (read().term && read().opts.regex) {
    let html = '', last = 0;
    for (const [start, end] of regexCached(value)) {
      html += esc(value.slice(last, start)) + `<mark>${esc(value.slice(start, end))}</mark>`;
      last = end;
    }
    return html + esc(value.slice(last));
  }
  const re = read().term && reTerm(true);
  if (!re) return esc(value);
  let html = '', last = 0;
  for (const match of value.matchAll(re)) {
    if (!match[0]) continue;
    html += esc(value.slice(last, match.index)) + `<mark>${esc(match[0])}</mark>`;
    last = match.index + match[0].length;
  }
  return html + esc(value.slice(last));
}

// The server's 40-character context can hide the hit below the sidebar's
// two-line clamp. Keep a short Unicode-safe lead-in; the title retains the
// full excerpt. Apply this to both newly created and reconciled rows.
export function sidebarSnippet(text: string) {
  const start = read().opts.regex ? regexCached(text)[0]?.[0] : reTerm(false)?.exec(text)?.index;
  if (start !== undefined) {
    const before = Array.from(text.slice(0, start));
    if (before.length > 8) text = '…' + before.slice(-8).join('') + text.slice(start);
  }
  return hl(text);
}

/** 在已渲染的 DOM 里给命中词套 <mark>, 走文本节点所以不会破坏标签。 */
export function markMatches(root: HTMLElement) {
  if (!read().term) return 0;
  if (read().opts.regex) { markRegexMatches(root); return 0; }
  const re = reTerm(true);
  if (!re) return 0;
  const messageBox = $('#msgs');
  const existing = messageBox && root !== messageBox && messageBox.contains(root)
    ? messageBox.querySelectorAll('mark').length : 0;
  const budget = Math.max(0, MARK_MAX - existing);
  // 只高亮正文: 折叠预览是正文副本, 高亮在那里会造成重复计数。
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: n => {
      const msg = n.parentElement!.closest('.msg');
      return !n.parentElement!.closest('[data-conversation-inner]') || n.parentElement!.closest('mark, .fold-preview, .katex')
        || !msg || !SEARCH_ROLES.has((msg as HTMLElement).dataset.role!)
        ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT;
    },
  });
  const targets: Text[] = [];
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    re.lastIndex = 0;                 // 带 g 的 test 会推进 lastIndex
    if (re.test(n.nodeValue!)) targets.push(n as Text);
  }
  let count = 0;
  for (const t of targets) {
    if (count >= budget) { bridge.write({markCapped: true}); break; }   // 搜 "a" 会生成上万节点, 会卡死
    const frag = document.createDocumentFragment();
    let last = 0, m;
    re.lastIndex = 0;
    while ((m = re.exec(t.nodeValue!))) {
      if (!m[0]) { re.lastIndex += re.unicode && t.nodeValue!.codePointAt(re.lastIndex)! > 0xffff ? 2 : 1; continue; }
      frag.append(t.nodeValue!.slice(last, m.index));
      const mk = document.createElement('mark');
      mk.textContent = m[0];
      frag.appendChild(mk);
      last = m.index + m[0].length;
      if (++count >= budget) { bridge.write({markCapped: true}); break; }
    }
    frag.append(t.nodeValue!.slice(last));
    t.parentNode!.replaceChild(frag, t);
  }
  return count;
}

export async function markRegexMatches(root: HTMLElement) {
  for (const node of [root, ...root.querySelectorAll<MatchElement>('.msg')]) (node as MatchElement)._applyRegexMatch?.();
  const generation = `${read().term}\0${JSON.stringify(read().opts)}`;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: n => {
      const msg = n.parentElement?.closest('.msg');
      return !n.parentElement?.closest('[data-conversation-inner]') || n.parentElement?.closest('mark, .fold-preview, .katex')
        || !msg || !SEARCH_ROLES.has((msg as HTMLElement).dataset.role!)
        ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT;
    },
  });
  const targets: Text[] = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) targets.push(node as Text);
  for (const node of targets) {
    const text = node.nodeValue!;
    const ranges = await regexMatches(text);
    if (generation !== `${read().term}\0${JSON.stringify(read().opts)}` || !root.isConnected) return;
    if (!node.isConnected || node.nodeValue !== text || node.parentElement!.closest('mark')) continue;
    const budget = MARK_MAX - ($('#msgs')?.querySelectorAll('mark').length || 0);
    if (budget <= 0) { bridge.write({markCapped: true}); break; }
    if (!ranges.length) continue;
    const fragment = document.createDocumentFragment();
    let last = 0;
    for (const [start, end] of ranges.slice(0, budget)) {
      fragment.append(text.slice(last, start));
      const mark = document.createElement('mark');
      mark.textContent = text.slice(start, end);
      fragment.append(mark);
      last = end;
    }
    fragment.append(text.slice(last));
    node.replaceWith(fragment);
  }
  updateMatchNav();
}

export function jumpMark(delta: number) {
  const marks = [...document.querySelectorAll<HTMLElement>('#msgs mark')].filter(
    m => m.checkVisibility ? m.checkVisibility({ visibilityProperty: true }) : m.offsetParent);
  if (!marks.length) return;
  bridge.write({cur: (read().cur + delta + marks.length) % marks.length});
  marks.forEach(m => m.classList.remove('cur'));
  marks[read().cur]!.classList.add('cur');
  marks[read().cur]!.scrollIntoView({ block: 'center', behavior: 'smooth' });
  state.navigation.text = `${read().cur + 1}/${marks.length}${read().markCapped ? '+' : ''} 处匹配`;
}

export function updateMatchNav({jump = false}: {jump?: boolean} = {}) {
 if (!document.querySelector('#mcount')) return 0
 const hits = document.querySelectorAll('#msgs mark').length
 state.navigation.text = hits ? `${hits}${read().markCapped ? '+' : ''} 处匹配` : '本页无匹配'
 const capped = read().markCapped || read().autoOpen >= AUTO_OPEN_MAX
 state.navigation.capped = capped
 state.navigation.title = capped ? `命中过多：只标注前 ${MARK_MAX} 处、自动展开前 ${AUTO_OPEN_MAX} 条，其余标 ● 需手动展开` : undefined
 if (jump && hits) jumpMark(1)
 return hits
}
export function generation() {return regexGeneration}
export function revision() {return state.revision}
