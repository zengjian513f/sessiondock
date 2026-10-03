import type { SearchOptions } from './types'
export function searchTerms(text: string) {
  const terms: string[] = [], chars = Array.from(text);
  let term = '', quoted = false;
  for (let i = 0; i < chars.length; i++) {
    const c = chars[i]!;
    if (quoted && c === '\\' && (chars[i + 1] === '"' || chars[i + 1] === '\\')) {
      term += chars[++i]!;
    } else if (c === '"') {
      quoted = !quoted;
    } else if (!quoted && /[\s\u0085]/u.test(c)) {
      if (term) { terms.push(term); term = ''; }
    } else {
      term += c!;
    }
  }
  if (term) terms.push(term);
  return [...new Set(terms)];
}

// 与 search.rs whole_word 一致：[\p{L}\p{N}_] 邻字挡住全词，但汉字/假名/谚文
// 与其它字母之间是边界。「无法识别的tag」能命中 tag，「猫猫」不能命中「猫」。
export function literalSource(term: string, opts: SearchOptions) {
  const src = term.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  if (!opts.word) return src;
  const cjk = '\\p{Script=Han}\\p{Script=Hiragana}\\p{Script=Katakana}\\p{Script=Hangul}\\p{Script=Bopomofo}';
  // JS has no class intersection; a letter outside those scripts is the other side.
  const nonCjkLetter = `(?:(?![${cjk}])\\p{L})`;
  const split = `(?<=[${cjk}])(?=${nonCjkLetter})|(?<=${nonCjkLetter})(?=[${cjk}])`;
  const before = `(?:(?<![\\p{L}\\p{N}_])|${split})`;
  const after = `(?:(?![\\p{L}\\p{N}_])|${split})`;
  return `${before}(?:${src})${after}`;
}

// Individual message previews and highlighting match any term, even when
// the session-level AND is satisfied by terms in different messages.
export function reTerm(term: string, opts: SearchOptions, global = false) {
  let src;
  if (opts.regex) {
    src = term;
    if (opts.word) src = `(?<!\\w)(?:${src})(?!\\w)`;
  } else {
    const terms = searchTerms(term).sort((a, b) => b.length - a.length);
    if (!terms.length) return null;
    src = terms.map(value => literalSource(value, opts)).join('|');
  }
  try {
    return new RegExp(src, (opts.case ? '' : 'i') + (global ? 'g' : '') + (opts.regex ? '' : 'u'));
  } catch {
    return null;   // 正则写到一半是常态, 不该炸掉整个界面
  }
}


export function matchesSearch(text: string, term: string, opts: SearchOptions, regexMatched: (text: string) => boolean) {
 if (opts.regex) return regexMatched(text)
 const terms = searchTerms(term)
 const test = (value: string) => new RegExp(literalSource(value, opts), opts.case ? 'u' : 'iu').test(text)
 return opts.mode === 'any' ? terms.some(test) : terms.every(test)
}
