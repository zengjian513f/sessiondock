// Streaming command output keeps its raw text nodes. Syntax ranges belong to
// immutable offsets within small text segments and are registered only in view.
const toolRangeAPI = !!(globalThis.CSS?.highlights && globalThis.StaticRange && globalThis.Highlight);
const visibleToolSyntax = new Set();
let toolSyntaxFrame = null;
function scheduleToolColors(piece) {
  if (!piece.isConnected || !piece.inView) return;
  visibleToolSyntax.add(piece);
  if (toolSyntaxFrame === null) toolSyntaxFrame = requestAnimationFrame(paintToolColors);
}
function paintToolColors() {
  toolSyntaxFrame = null;
  const start = performance.now();
  for (const piece of visibleToolSyntax) {
    visibleToolSyntax.delete(piece);
    if (piece.isConnected && piece.inView) piece.paintColors();
    if (performance.now() - start >= 4) break;
  }
  if (visibleToolSyntax.size) toolSyntaxFrame = requestAnimationFrame(paintToolColors);
}
const toolColorObserver = globalThis.IntersectionObserver && new IntersectionObserver(entries => {
  for (const {target, isIntersecting} of entries) {
    target.inView = isIntersecting;
    if (isIntersecting) scheduleToolColors(target);
    else target.dropColors();
  }
});
class ToolSyntaxPiece extends HTMLElement {
  constructor() {
    super();
    this.colors = [];
    this.painted = 0;
    this.registered = [];
    this.lines = 0;
    this.length = 0;
  }
  connectedCallback() {
    if (toolColorObserver) toolColorObserver.observe(this);
    else { this.inView = true; scheduleToolColors(this); }
  }
  disconnectedCallback() {
    toolColorObserver?.unobserve(this);
    this.inView = false;
    this.dropColors();
  }
  dropColors() {
    visibleToolSyntax.delete(this);
    this.changed?.disconnect();
    for (const [group, range] of this.registered) group.delete(range);
    this.registered.length = 0;
    this.painted = 0;
  }
  paintColors() {
    if (this.painted === this.colors.length) return;
    if (!toolRangeAPI) {
      if (this.querySelector('mark') || (!getSelection().isCollapsed && getSelection().containsNode(this, true))) return;
      const source = this.textContent, fragment = document.createDocumentFragment();
      let at = 0;
      for (const [start, end, color] of this.colors) {
        fragment.append(source.slice(at, start));
        fragment.append(el('span', 'hljs-' + (color === 'builtin' ? 'built_in' : color), source.slice(start, end)));
        at = end;
      }
      fragment.append(source.slice(at));
      this.replaceChildren(fragment);
    } else {
      // Search can split this piece into several text nodes. Preserve its
      // native marks and rebuild only this piece when those marks change.
      const walker = document.createTreeWalker(this, NodeFilter.SHOW_TEXT);
      let node = walker.nextNode(), base = 0;
      for (let i = this.painted; i < this.colors.length; i++) {
        const [start, end, color] = this.colors[i];
        while (node && base < end) {
          const next = base + node.length;
          if (next > start && !node.parentElement.closest('mark')) {
            const name = 'sd-tool-' + color;
            let group = CSS.highlights.get(name);
            if (!group) { group = new Highlight(); CSS.highlights.set(name, group); }
            const range = new StaticRange({startContainer: node, startOffset: Math.max(0, start - base),
              endContainer: node, endOffset: Math.min(end, next) - base});
            group.add(range);
            this.registered.push([group, range]);
          }
          if (next > end) break;
          base = next; node = walker.nextNode();
        }
      }
      this.changed ??= new MutationObserver(() => { this.dropColors(); scheduleToolColors(this); });
      this.changed.observe(this, {subtree: true, childList: true});
    }
    this.painted = this.colors.length;
  }
}
customElements.define('sd-tool-syntax', ToolSyntaxPiece);

function initToolStream(pre) {
  pre._toolStream = {source: '', sent: 0, ready: 0, continuation: '', pieces: [], tail: null,
    header: '', minus: -1, lineCount: 0, lastNonemptyLine: 0, final: false, diff: null};
}
function appendToolStream(pre, delta, final = false) {
  const state = pre._toolStream;
  const start = state.source.length;
  state.source += delta;
  if (delta || final) state.retries = 0;
  state.final = final;
  let offset = start;
  let foundDiff = false;
  const pieces = document.createDocumentFragment();
  let buffered = '';
  const flushText = () => {
    if (!buffered) return;
    const piece = state.tail;
    if (piece.lastChild?.nodeType === Node.TEXT_NODE) piece.lastChild.appendData(buffered);
    else piece.append(buffered);
    buffered = '';
  };
  for (const [line] of delta.matchAll(/[^\n]*\n|[^\n]+$/g)) {
    if (!state.diff) {
      let cursor = 0;
      while (cursor < line.length) {
        let piece = state.tail;
        if (!piece || piece.lines >= 48 || piece.length >= 4096) {
          flushText();
          // Isolate whole line groups so coloring the tail never lays out the
          // entire scrollback. Pieces split inside one long line stay inline.
          if (piece && (piece.start === 0 || state.source[piece.start - 1] === '\n')
              && state.source[offset + cursor - 1] === '\n') {
            piece.classList.add('tool-lines');
            piece.style.setProperty('--syntax-lines', piece.lines);
          }
          piece = document.createElement('sd-tool-syntax');
          piece.start = offset + cursor;
          state.pieces.push(piece);
          state.tail = piece;
          pieces.append(piece);
        }
        let until = Math.min(line.length, cursor + 4096 - piece.length);
        if (until < line.length && /[\uD800-\uDBFF]/.test(line[until - 1])) until++;
        const text = line.slice(cursor, until);
        buffered += text;
        piece.length += text.length;
        if (text.endsWith('\n')) piece.lines++;
        cursor = until;
      }
      state.header += line.slice(0, Math.max(0, 12 - state.header.length));
      if (state.header.startsWith('diff --git ') || (state.minus >= 0 && state.lineCount - state.minus <= 3
          && state.header.startsWith('+++ '))) foundDiff = true;
      if (state.header.startsWith('--- ')) state.minus = state.lineCount;
    }
    offset += line.length;
    if (line !== '\n') state.lastNonemptyLine = state.lineCount + 1;
    if (line.endsWith('\n')) { state.ready = offset; state.lineCount++; state.header = ''; }
  }
  flushText();
  if (pieces.childElementCount) pre.append(pieces);
  if (final) state.ready = state.source.length;
  if (state.diff) {
    appendToolDiff(pre, delta, final);
  } else if (foundDiff) {
    pre.textContent = state.source;
    paintToolOutputDiff(pre);
    state.pieces.length = 0; state.tail = null;
    pre.dataset.syntaxDone = '1';
    paintSyntax(pre);
  } else if (state.ready > state.sent) {
    delete pre.dataset.syntaxDone;
    syntaxQueue.add(pre);
    syntaxGenerations.set(pre, renderSeq);
    scheduleSyntax();
  } else pre.dataset.syntaxDone = '1';
}
function streamSyntaxJob(pre) {
  const state = pre._toolStream;
  if (state.diff || state.ready <= state.sent) return null;
  return {code: pre, state, start: state.sent, end: state.ready, block: {
    kind: 'stream', source: state.source.slice(state.sent, state.ready),
    continuation: state.continuation, final: state.final, command: state.continuation ? undefined : pre._outputCommand,
  }};
}
function applyToolStream(job, result) {
  const {code, state, start, end} = job;
  if (!code.isConnected || state !== code._toolStream || state.diff || start !== state.sent) return;
  if (result?.reset && start) {
    // An evicted grammar checkpoint (including a server restart) requires one
    // re-seed from the original text. The visible source stays in place.
    state.sent = 0; state.continuation = '';
    state.pieces = [...code.querySelectorAll(':scope > sd-tool-syntax')];
    for (const piece of state.pieces) { piece.dropColors(); piece.colors.length = 0; }
    syntaxQueue.add(code);
    return;
  }
  if (!result?.ranges) return;
  let i = 0, consumed = 0;
  for (const piece of state.pieces) {
    if (piece.start >= end) break;
    const limit = piece.start + piece.length;
    while (i < result.ranges.length) {
      const [from, to, color] = result.ranges[i];
      if (start + from >= limit) break;
      if (start + to > piece.start) piece.colors.push([Math.max(0, start + from - piece.start),
        Math.min(limit, start + to) - piece.start, color]);
      if (start + to > limit) break;
      i++;
    }
    scheduleToolColors(piece);
    if (piece !== state.tail && limit <= end) consumed++;
  }
  if (consumed) state.pieces.splice(0, consumed);
  state.sent = end;
  state.retries = 0;
  state.continuation = result.continuation || '';
  if (result.languages?.length) {
    const languages = new Set([...(code.dataset.syntaxLanguages || '').split(',').filter(Boolean), ...result.languages]);
    code.dataset.syntaxLanguages = [...languages].join(',');
    if (languages.size === 1) code.dataset.codeLanguage = [...languages][0];
    else delete code.dataset.codeLanguage;
  }
  if (state.ready > end) syntaxQueue.add(code);
  else code.dataset.syntaxDone = '1';
}

// A failed request leaves its text and offsets intact. Retry a quiet tail as
// well; subsequent deltas naturally coalesce with any outstanding retry.
function retryToolStream(job) {
  const {code, state} = job;
  if (!state || code._toolStream !== state || !code.isConnected || state.diff || state.sent !== job.start
      || state.retries >= 2 || syntaxQueue.has(code) || deferFoldedToolSyntax(code)) return;
  const attempt = ++state.retries;
  setTimeout(() => {
    if (!code.isConnected || code._toolStream !== state || state.sent !== job.start || state.diff) return;
    syntaxQueue.add(code);
    scheduleSyntax();
  }, 300 * attempt);
}

// outputDelta is append-only by protocol. A completion may instead replace
// stdout or the command; let the ordinary item renderer handle that correction.
function updateCommandOutput(card, message, appended) {
  const previous = card._sdkToolMessage;
  if (!previous?.streamOutput || previous.text !== message.text || previous.summary !== message.summary) return false;
  const entry = card.querySelector('.tool-entry'), result = message.result;
  if (!entry || !result) return false;
  const source = result.text || '';
  let pre = entry.querySelector('pre.tool-out');
  const oldSource = pre?._toolStream?.source ?? '';
  if ((pre && !pre._toolStream) || (!appended && !source.startsWith(oldSource))) return false;
  if (!pre && source.trim()) pre = addClippedPre(entry, 'tool-out', '', '', true);
  if (pre) {
    pre._outputCommand = message.text;
    appendToolStream(pre, source.slice(oldSource.length), !message.streamOutput);
    const actions = pre.parentNode.nextElementSibling;
    if ((source.length > OUT_CHARS || pre._toolStream.lastNonemptyLine > OUT_LINES) && !actions.querySelector('.more'))
      addToolExpansion(pre, actions);
    if (pre.classList.contains('collapsed-output') && !pre.closest('.text-scroll-active')) queuePreviewTail(pre);
    pre.classList.toggle('err', !!result.error);
  }
  entry.dataset.result = '1';
  entry._toolComplete = ConversationUI.toolComplete(message);
  const status = entry.querySelector('.tool-status');
  if (status) {
    status.classList.toggle('err', !!result.error);
    status.textContent = message.streamOutput ? '运行中…' : toolResultLabel(result, entry._toolComplete,
      {chars: source.length, lines: pre?._toolStream?.lastNonemptyLine ?? outputStats(source).lines});
  }
  return true;
}
