

import * as SessionUi from '../../migration/session-ui'

import * as Overlays from '../../migration/overlays'

export function createMarkdownRuntime({dom,core,mediaRuntime,sidebarGestures,capabilities}) {
const CLIP = 4000;

const AUTO_OPEN_MAX = 40;

const MARK_MAX = 3000;

const clipText = t => t.length > CLIP ? t.slice(0, CLIP) + '\n… (点下方按钮展开全文)' : t;

function md(text, full, media = [], context = {}) {
  const source = full ? text : clipText(text);
  const lines = source.split('\n');
  const output = [], prose = [];
  const flush = () => {
    if (!prose.length) return;
    output.push(blocks(prose.join('\n'), media, context));
    prose.length = 0;
  };
  for (let i = 0; i < lines.length;) {
    // 只有独立行上的 Markdown 围栏才是代码块。旧的 split(/```/)
    // 会把句子里提到的 ```python 也当成开头，后半条消息全吞进 pre。
    const open = lines[i].match(/^\s{0,3}(`{3,}|~{3,})[^\S\n]*(.*)$/);
    if (!open || (open[1][0] === '`' && open[2].includes('`'))) {
      prose.push(lines[i++]);
      continue;
    }
    const marker = open[1][0], width = open[1].length;
    let close = i + 1;
    for (; close < lines.length; close++) {
      const found = lines[close].match(/^\s{0,3}(`+|~+)\s*$/);
      if (found && found[1][0] === marker && found[1].length >= width) break;
    }
    // 未闭合围栏只在“展开全文”预览恰好截断时按代码处理；
    // 原文本本身未闭合时保留原样，不再把整条消息误判为代码。
    if (close === lines.length && (full || text.length <= CLIP)) {
      prose.push(lines[i++]);
      continue;
    }
    flush();
    const info = open[2].trim();
    const language = info.match(/^[\w+.-]+/)?.[0] || '';
    const code = lines.slice(i + 1, close).join('\n');
    output.push(`<pre><code class="code-block" data-code-lang="${dom().esc(language)}">${dom().esc(code)}</code></pre>`);
    i = close < lines.length ? close + 1 : close;
  }
  flush();
  return output.join('') || '<p></p>';
}

const RE_LIST = /^\s*([-*+]|\d+[.)])\s+/;

const RE_HEAD = /^\s*(#{1,6})\s+(.*)$/;

const RE_QUOTE = /^\s*>\s?/;

const RE_HR = /^\s*([-*_])\s*(\1\s*){2,}$/;

const isSep = s => /^\s*\|?[\s:|-]+\|[\s:|-]*$/.test(s) && s.includes('-');

const cells = s => s.trim().replace(/^\||\|$/g, '').split('|').map(x => x.trim());

const isTable = (ls, i) => ls[i].includes('|') && i + 1 < ls.length && isSep(ls[i + 1]);

function blocks(src, media = [], context = {}) {
  const ls = src.split('\n');
  let out = '', i = 0;
  while (i < ls.length) {
    const line = ls[i];
    if (!line.trim()) { i++; continue; }

    if (isTable(ls, i)) {
      const head = cells(line);
      const align = cells(ls[i + 1]).map(c =>
        /^:-+:$/.test(c) ? 'center' : /-+:$/.test(c) ? 'right' : 'left');
      const at = j => `style="text-align:${align[j] || 'left'}"`;
      i += 2;
      const rows = [];
      while (i < ls.length && ls[i].trim() && ls[i].includes('|')) rows.push(cells(ls[i++]));
      out += `<div class="tw"><table><thead><tr>${head.map((c, j) => `<th ${at(j)}>${inline(c, media, context)}</th>`).join('')}</tr></thead>`
        + `<tbody>${rows.map(r => `<tr>${r.map((c, j) => `<td ${at(j)}>${inline(c, media, context)}</td>`).join('')}</tr>`).join('')}</tbody></table></div>`;
      continue;
    }

    const h = line.match(RE_HEAD);
    if (h) { out += `<h3 class="h${h[1].length}">${inline(h[2], media, context)}</h3>`; i++; continue; }

    if (RE_HR.test(line)) { out += '<hr>'; i++; continue; }

    if (RE_LIST.test(line)) {
      const tag = /^\s*\d/.test(line) ? 'ol' : 'ul';
      const items = [];
      while (i < ls.length && RE_LIST.test(ls[i])) {
        let item = ls[i++].replace(RE_LIST, '');
        while (i < ls.length && ls[i].trim() && !RE_LIST.test(ls[i]) && /^\s{2,}/.test(ls[i])) {
          item += '\n' + ls[i++].trim();     // 续行并入当前条目
        }
        items.push(`<li>${inline(item, media, context)}</li>`);
      }
      out += `<${tag}>${items.join('')}</${tag}>`;
      continue;
    }

    if (RE_QUOTE.test(line)) {
      const qs = [];
      while (i < ls.length && RE_QUOTE.test(ls[i])) qs.push(ls[i++].replace(RE_QUOTE, ''));
      out += `<blockquote>${inline(qs.join('\n'), media, context)}</blockquote>`;
      continue;
    }

    const para = [];
    while (i < ls.length && ls[i].trim() && !RE_LIST.test(ls[i]) && !RE_HEAD.test(ls[i])
           && !RE_QUOTE.test(ls[i]) && !RE_HR.test(ls[i]) && !isTable(ls, i)) {
      para.push(ls[i++]);
    }
    if (para.length) out += `<p>${inline(para.join('\n'), media, context)}</p>`;
    else i++;                                 // 兜底: 保证 i 一定前进
  }
  return out;
}

const RE_MD_IMAGE = /!\[([^\]]*)\]\(\s*(<[^>]+>|[^\s)]+)(?:\s+["'][^"']*["'])?\s*\)/g;

const RE_CODE_SPAN = /(^|[^`])(`+)(?!`)([^\n]*?)(?<!`)\2(?!`)/g;

const RE_REFERENCE = /(?<![A-Za-z0-9_@/\\:.-])(?:(?:https?:\/\/|www\.)[^\s<>"'`\u0000，。；、！？]+|[A-Za-z]:[/\\][^\s<>"'`\u0000，。；、！？()[\]{}]*|(?:~\/|\.\.?\/|\/|[A-Za-z0-9_.-]+\/)[^\s<>"'`\u0000，。；、！？()[\]{}]+|[A-Za-z0-9_-][A-Za-z0-9_.-]*\.[A-Za-z][A-Za-z0-9_-]*(?::\d+(?::\d+)?|#L\d+(?:C\d+)?)?)/gi;

const isWindowsDrivePath = path => /^[A-Za-z]:[/\\]/.test(path);

function fileParentDirectory(path) {
  const slash = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
  // A drive root includes its separator: X: alone is drive-relative.
  return isWindowsDrivePath(path) && slash === 2 ? path.slice(0, 3) : path.slice(0, slash) || '/';
}

function trimReference(raw) {
  let ref = raw.replace(/[.,;:!?]+$/, '');
  for (const [left, right] of [['(', ')'], ['[', ']']]) {
    while (ref.endsWith(right) && ref.split(right).length > ref.split(left).length) ref = ref.slice(0, -1);
  }
  return ref;
}

function referenceLink(ref, label, context, explicit = false) {
  let href = '';
  if (/^(https?:\/\/|www\.)/i.test(ref)) {
    try {
      const url = new URL(/^www\./i.test(ref) ? 'https://' + ref : ref);
      if (!['http:', 'https:'].includes(url.protocol)) return '';
      href = url.href;
    } catch { return ''; }
  } else {
    // Browser file:// navigation cannot reach a remote node. Resolve only
    // references present in this session, through its authenticated API.
    const withoutLine = ref.replace(/(?::\d+(?::\d+)?|#L\d+(?:C\d+)?)$/, '');
    const windowsDrive = isWindowsDrivePath(withoutLine);
    if (!context.uid || (!windowsDrive && /^[a-z][a-z0-9+.-]*:/i.test(withoutLine)) || ref.startsWith('//')) return '';
    if (/^[A-Z0-9]+(?:\/[A-Z0-9]+)+$/.test(ref)) return '';
    // Rendering is lexical only. Resolve history, existence and ambiguity on
    // explicit navigation/menu actions, never once per render or SSE update.
    const filename = /^[^\s/<>"'`=;|{}\[\]]+\.[a-zA-Z][\w.-]*$/.test(withoutLine);
    const path = windowsDrive || /^(?:~\/|\.\.?\/|\/)[^\n]+$/.test(withoutLine)
      || (!/[\s<>"'`=;|{}\[\]]/.test(withoutLine) && withoutLine.includes('/')
          && (withoutLine.endsWith('/') || /^[^/]+\.[a-zA-Z][\w.-]*$/.test(withoutLine.split('/').pop())))
      || filename;
    if (!path && !explicit) return '';
    const query = new URLSearchParams({uid: context.uid, ref});
    if (context.agent) query.set('agent', context.agent);
    href = core.environment.appUrl('/api/session/file') + '?' + query;
    const open = core.environment.appUrl('file.html') + '?' + query + '&open=1';
    return `<a href="${dom().esc(open)}" data-file-ref="${dom().esc(ref)}" data-file-href="${dom().esc(href)}" target="_blank" rel="noopener noreferrer">${label}</a>`;
  }
  return `<a href="${dom().esc(href)}" data-reference-kind="web" target="_blank" rel="noopener noreferrer">${label}</a>`;
}

function inline(s, media = [], context = {}) {
  s = String(s).replace(/\u0000/g, '');
  const codeSpans = [], codeLabels = [], codeText = [];
  s = s.replace(RE_CODE_SPAN, (_, prefix, _ticks, raw) => {
    // Markdown 代码跨度允许内容中出现更长的反引号串，例如用单反引号
    // 包住 ```python。先占位再处理图片/粗体，避免代码内容被二次解析。
    const content = raw.startsWith(' ') && raw.endsWith(' ') && /\S/.test(raw)
      ? raw.slice(1, -1) : raw;
    const code = `<code>${dom().esc(content)}</code>`;
    codeText.push(content);
    codeLabels.push(code);
    codeSpans.push(code);
    return `${prefix}\u0000CODE${codeSpans.length - 1}\u0000`;
  });
  const images = [];
  s = s.replace(RE_MD_IMAGE, (_, alt, raw) => {
    const ref = raw.replace(/^<|>$/g, '');
    const found = media.find(x => x.ref === ref);
    const src = found?.src || ref;
    const html = mediaRuntime().imageHtml({ ...(found || {}), src, alt: alt || found?.alt || '图片' }, true);
    if (!html) return `[图片: ${alt || ref}]`;
    images.push(html);
    return `\u0000IMG${images.length - 1}\u0000`;
  });
  const links = [];
  const keepLink = html => {
    links.push(html);
    return `\u0000LINK${links.length - 1}\u0000`;
  };
  const emphasis = text => text
    .replace(/\*\*([^*\n]+)\*\*/g, '<b>$1</b>')
    .replace(/(^|[^*\w])\*([^*\n]+)\*(?!\w)/g, '$1<i>$2</i>');
  // Standard Markdown links display their label only. Keep the destination
  // in link metadata for navigation/menu actions, never append it to the label.
  s = s.replace(/\[([^\]\n]+)\]\(\s*(<[^>\n]+>|(?:[^\s()]|\([^\s()]*\))+)(?:\s+["']([^"']*)["'])?\s*\)/g,
    (_raw, label, target, title) => {
      const content = emphasis(dom().esc(label)).replace(/\u0000CODE(\d+)\u0000/g,
        (_, i) => codeLabels[+i] || '');
      const ref = target.replace(/^<|>$/g, '');
      let html = referenceLink(ref, content, context, true) || content;
      if (title !== undefined && html.startsWith('<a ')) html = html.replace('<a ', `<a title="${dom().esc(title)}" `);
      return keepLink(html);
    });
  const linkCandidate = (raw, offset, source) => {
    // Do not link a suffix of a scheme, identifier or email address.
    if (offset && /[\w@/\\:.-]/.test(source[offset - 1])) return raw;
    const ref = trimReference(raw);
    const html = referenceLink(ref, dom().esc(ref), context);
    return html ? keepLink(html) + raw.slice(ref.length) : raw;
  };
  // Parentheses opt prose into linkification. Code spans are explicit
  // references too, including when they appear outside parentheses.
  const parenthesized = (part, explicit) => {
    // A complete target may itself contain balanced parentheses. Do not split
    // a URL or a filename such as report(final).pdf into several links.
    // Preserve every delimiter, space and optional Markdown title. Only wrap
    // the existing target substring; never synthesize a label or new text.
    const match = part.match(/^(\s*)(<([^<>\n]+)>|(?:[^\s()]|\([^\s()]*\))+)(\s+(?:["'][^"']*["'])\s*|\s*)$/);
    if (match && !match[2].includes('\u0000')) {
      const ref = match[3] || match[2];
      const html = referenceLink(ref, dom().esc(ref), context, explicit);
      if (html) return match[1] + (match[3] ? '<' : '') + keepLink(html)
        + (match[3] ? '>' : '') + match[4];
    }
    return part.replace(/\u0000CODE(\d+)\u0000/g, (raw, i) => {
      const code = codeLabels[+i];
      return keepLink(referenceLink(codeText[+i], code, context) || code);
    }).replace(RE_REFERENCE, linkCandidate);
  };
  const parts = [], stack = [];
  let start = 0, open = -1;
  for (let i = 0; i < s.length; i++) {
    if (s[i] === '(' || s[i] === '（') {
      if (!stack.length) open = i;
      stack.push(s[i] === '(' ? ')' : '）');
    } else if (stack.length && s[i] === stack[stack.length - 1]) {
      stack.pop();
      if (!stack.length) {
        parts.push(s.slice(start, open + 1), parenthesized(s.slice(open + 1, i), s[open - 1] === ']'), s[i]);
        start = i + 1;
      }
    }
  }
  parts.push(s.slice(start));
  s = parts.join('');
  s = s.replace(/\u0000CODE(\d+)\u0000/g, (raw, i) =>
    keepLink(referenceLink(codeText[+i], codeLabels[+i], context) || codeLabels[+i]));
  return emphasis(dom().esc(s))
    .replace(/\u0000LINK(\d+)\u0000/g, (_, i) => links[+i] || '')
    .replace(/\u0000CODE(\d+)\u0000/g, (_, i) => codeSpans[+i] || '')
    .replace(/\u0000IMG(\d+)\u0000/g, (_, i) => images[+i] || '');
}
function start(){
Overlays.mountFiles({fetch:(...args)=>core.network.fetch(...args),appUrl:core.environment.appUrl, allows:capabilities.allows,
  closeItemMenu:sidebarGestures().closeItemMenu, alert:SessionUi.appAlert});
}
return {CLIP,AUTO_OPEN_MAX,MARK_MAX,clipText,md,RE_LIST,RE_HEAD,RE_QUOTE,RE_HR,isSep,cells,isTable,blocks,RE_MD_IMAGE,RE_CODE_SPAN,RE_REFERENCE,isWindowsDrivePath,fileParentDirectory,trimReference,referenceLink,inline,start};
}
