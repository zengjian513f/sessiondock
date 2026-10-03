
import * as Search from '../../migration/search'

import * as Conversation from '../../migration/conversation'

import * as Overlays from '../../migration/overlays'

export function createSyntaxRuntime({dom,conversationRenderer,core}) {
function clearSyntaxPaint(node) {
  delete node.dataset.syntaxDone;
  delete node.dataset.detected;
  delete node.dataset.codeLanguage;
  delete node.dataset.syntaxLanguages;
  node.classList.remove('hljs');
  for (const cls of [...node.classList]) if (cls.startsWith('language-')) node.classList.remove(cls);
}

function paintToolOutputDiff(pre) {
  if (!(pre instanceof HTMLElement) || pre.querySelector(':scope > .tool-diff-line')) return;
  const lines = pre.textContent.split('\n');
  const first = lines.findIndex(line => line.startsWith('diff --git '));
  const unified = first >= 0 ? first : lines.findIndex((line, i) =>
    line.startsWith('--- ') && lines.slice(i + 1, i + 4).some(x => x.startsWith('+++ ')));
  if (unified < 0) return;
  const path = Conversation.planning.diffCodePath(lines.slice(unified));
  const fragment = document.createDocumentFragment();
  lines.forEach((line, i) => {
    const row = dom().el('span', 'tool-diff-line');
    const kind = i >= unified ? Conversation.planning.diffKind(line) : '';
    if (kind) row.classList.add(kind);
    const parts = path && Conversation.planning.diffCodeParts(line, kind);
    if (parts) {
      row.classList.add('code');
      row.appendChild(dom().el('i', '', parts.marker));
      const code = dom().el('code', '', parts.source || ' ');
      code.dataset.codePath = path;
      row.appendChild(code);
    } else {
      row.textContent = line || ' ';
    }
    fragment.appendChild(row);
  });
  pre.replaceChildren(fragment);
}

let syntaxWorker = null, syntaxBusy = false, syntaxFrame = null;

const syntaxQueue = new Set();

const syntaxGenerations = new WeakMap();

function scheduleSyntax() {
  if (syntaxBusy || syntaxFrame !== null || !syntaxQueue.size) return;
  syntaxFrame = requestAnimationFrame(() => {
    syntaxFrame = null;
    let code;
    const waiting = [];
    for (const candidate of syntaxQueue) {
      syntaxQueue.delete(candidate);
      if (candidate.isConnected && !candidate.dataset.syntaxDone) { code = candidate; break; }
      // Large histories yield while their nodes still live in a fragment.
      // Keep those jobs until publication, but discard abandoned render plans.
      if (!candidate.dataset.syntaxDone && syntaxGenerations.get(candidate) === conversationRenderer().renderSeq
          && candidate.getRootNode().nodeType === Node.DOCUMENT_FRAGMENT_NODE) waiting.push(candidate);
    }
    for (const candidate of waiting) syntaxQueue.add(candidate);
    if (!code) return;         // Fragment publication schedules us; do not spin every frame.
    syntaxBusy = true;
    const source = code.textContent;
    const language = code.dataset.codeLang || '', path = code.dataset.codePath || '';
    const kind = code.matches('code.tool-command') ? 'command'
      : code.matches('pre.tool-out') ? 'tool' : 'code';
    let timer;
    const finish = result => {
      clearTimeout(timer);
      // An expanded/replaced preview must never receive an older worker reply.
      if (code.isConnected && code.textContent === source
          && (code.dataset.codeLang || '') === language && (code.dataset.codePath || '') === path) {
        code.dataset.syntaxDone = '1';
        if (result?.html) {
          code.innerHTML = result.html;
          code.classList.add('hljs');
          if (result.language) code.classList.add(`language-${result.language}`);
          if (result.languages?.length) code.dataset.syntaxLanguages = result.languages.join(',');
          if (result.detected) code.dataset.detected = result.language;
          const host = code.tagName === 'PRE' ? code
            : (code.classList.contains('code-block') && code.parentElement?.tagName === 'PRE'
                ? code.parentElement : null);
          if (result.language && host) host.dataset.codeLanguage = result.language;
          // Syntax arrives after message search decoration now. Reapply the
          // current query to the new text nodes instead of losing its marks.
          if (core.state.search.term) { Search.markMatches(code); Search.updateMatchNav(); }
        }
      }
      syntaxBusy = false;
      scheduleSyntax();
    };
    const failed = () => {
      syntaxWorker?.terminate();
      syntaxWorker = null;
      finish(null);
    };
    try {
      syntaxWorker ||= new Worker(Overlays.assets.url('syntax-worker.js'), {type: 'module'});
      syntaxWorker.onmessage = event => finish(event.data);
      syntaxWorker.onerror = failed;
      // Optional decoration must not hold up other blocks indefinitely. The
      // complete original text stays readable even when a grammar stalls.
      timer = setTimeout(failed, 5000);
      syntaxWorker.postMessage({source, kind, language, path});
    } catch { failed(); }
  });
}

function paintSyntax(root = document) {
  const select = selector => [
    ...(root.matches?.(selector) ? [root] : []),
    ...root.querySelectorAll(selector),
  ];
  const blocks = select('code.code-block:not([data-syntax-done])');
  const summaries = select('code.tool-command:not([data-syntax-done])');
  const tools = select('pre.tool-out:not([data-syntax-done])').filter(
    pre => !pre.querySelector(':scope > .tool-diff-line'));
  const diffLines = select(
    '.diff-line > code[data-code-path]:not([data-syntax-done]), '
    + '.tool-diff-line > code[data-code-path]:not([data-syntax-done])');
  const nodes = [...new Set([...blocks, ...summaries, ...tools, ...diffLines])];
  if (!nodes.length) return;
  for (const code of nodes) {
    syntaxQueue.add(code);
    syntaxGenerations.set(code, conversationRenderer().renderSeq);
  }
  scheduleSyntax();
}
return {clearSyntaxPaint,paintToolOutputDiff,get syntaxWorker(){return syntaxWorker},get syntaxBusy(){return syntaxBusy},get syntaxFrame(){return syntaxFrame},syntaxQueue,syntaxGenerations,scheduleSyntax,paintSyntax};
}
