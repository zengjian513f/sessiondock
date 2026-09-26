import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {test} from 'node:test';
import vm from 'node:vm';

const read = name => readFileSync(new URL(`../legacy-web/${name}`, import.meta.url), 'utf8');
const capabilitiesSource = read('capabilities.js');
const appSource = read('app.js');
const disabled = {backend: 'rust', read_only: true, live: false,
  outbox: false, audit: false, search: false, files: false};

test('literal search supports AND/OR, quoted phrases and safe per-term highlighting', () => {
  const S = {term: '部署 失败', opts: {mode: 'all', case: false, word: false, regex: false}};
  const context = vm.createContext({S, esc: value => String(value)
    .replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;')});
  for (const name of ['searchTerms', 'literalSource', 'reTerm', 'hasTerm', 'matchesSearch', 'hl']) {
    loadFunction(context, name);
  }
  assert.equal(context.matchesSearch('失败\n部署'), true);
  assert.equal(context.matchesSearch('部署完成'), false);
  assert.equal(context.hasTerm('部署完成'), true, 'message previews match one term of a session AND');
  S.opts.mode = 'any';
  assert.equal(context.matchesSearch('部署完成'), true);
  S.opts.mode = 'all';
  S.term = '"部署 失败" 重启';
  assert.equal(context.matchesSearch('重启\n部署 失败'), true);
  assert.equal(context.matchesSearch('重启\n部署\n失败'), false);
  assert.deepEqual(Array.from(context.searchTerms('  "" 部署\t失败 部署')), ['部署', '失败']);
  assert.deepEqual(Array.from(context.searchTerms('"部署 失败')), ['部署 失败']);
  assert.deepEqual(Array.from(context.searchTerms('AND OR')), ['AND', 'OR']);
  assert.deepEqual(Array.from(context.searchTerms('a\u0085b\ufeffc')), ['a', 'b', 'c']);
  assert.deepEqual(Array.from(context.searchTerms(String.raw`"say \"hi\" at C:\\tmp" x`)), ['say "hi" at C:\\tmp', 'x']);
  S.term = 'Kelvin session';
  assert.equal(context.matchesSearch('KELVIN\nſession'), true);
  S.opts.case = true;
  assert.equal(context.matchesSearch('KELVIN\nſession'), false);
  S.opts.case = false;
  S.opts.word = true;
  S.term = '猫 部署';
  assert.equal(context.matchesSearch('猫猫 部署'), false);
  assert.equal(context.matchesSearch('猫\n部署'), true);
  S.opts.word = false;
  S.term = '<img> x+y';
  assert.equal(context.hl('<img> & x+y'), '<mark>&lt;img&gt;</mark> &amp; <mark>x+y</mark>');
  assert.equal(context.hl(undefined), 'undefined', 'missing title retains the legacy fallback');
  S.opts.regex = true;
  S.opts.mode = 'any';
  S.term = 'foo bar';
  assert.equal(context.matchesSearch('foo\nbar'), false, 'regex ignores boolean mode');
  S.term = 'foo|bar';
  assert.equal(context.matchesSearch('bar'), true);
  S.term = '\\_';
  assert.equal(context.matchesSearch('_'), true, 'advanced regex retains legacy JS syntax');
});

function contextWithCapabilities(value, globals = {}) {
  const meta = value === undefined ? null
    : {content: typeof value === 'string' ? value : JSON.stringify(value)};
  const context = vm.createContext({document: {querySelector: () => meta}, ...globals});
  vm.runInContext(capabilitiesSource, context);
  return context;
}

// Execute the real small top-level functions without inventing a second
// implementation or loading the entire DOM-heavy application in a fake browser.
function loadFunction(context, name, source = appSource) {
  const match = new RegExp(`^(?:async )?function ${name}\\(`, 'm').exec(source);
  assert.ok(match, `Function ${name} exists`);
  const end = source.indexOf('\n}\n', match.index);
  assert.ok(end > match.index, `Function ${name} ends`);
  vm.runInContext(source.slice(match.index, end + 2), context);
  return context[name];
}

test('local media tokens render without granting remote image requests', () => {
  const globals = {HUB_MODE: false, URL, appUrl: path => path};
  const rust = contextWithCapabilities({...disabled, media: true, media_remote: false}, globals);
  const safe = loadFunction(rust, 'safeMediaSrc');
  const token = '/api/media/' + 'a'.repeat(32);
  assert.equal(safe(token), token);
  for (const source of ['https://example.com/pixel.png', 'http://example.com/pixel.png',
    'data:image/png;base64,eA==', 'file:///private/image.png', '/api/media/not-a-token', '//example.com/pixel.png']) {
    assert.equal(safe(source), '', source);
  }
  const python = contextWithCapabilities(undefined, globals);
  assert.equal(loadFunction(python, 'safeMediaSrc')('https://example.com/pixel.png'), 'https://example.com/pixel.png');
});

test('a page without declared capabilities uses SessionDock defaults', () => {
  const {SessionDockCapabilities: caps} = contextWithCapabilities();
  assert.equal(caps.declared, false);
  assert.equal(caps.namespace, 'sessiondock.');
  for (const name of ['audit', 'live', 'outbox', 'files', 'search', 'terminal', 'watch']) {
    assert.equal(caps.allows(name), true, name);
  }
});

test('per-image errors are escaped visible explanations, never image requests', () => {
  const context = contextWithCapabilities({...disabled, media: true, media_remote: false}, {
    esc: value => String(value).replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;'),
    safeMediaSrc: () => { throw new Error('error descriptor must not access a source'); }
  });
  const render = loadFunction(context, 'imageHtml');
  for (const inline of [false, true]) {
    const html = render({src: 'https://example.invalid/image.png', error: {message: '<img src=x onerror=alert(1)>'}}, inline);
    assert.match(html, /media-error/);
    assert.match(html, /role="status"/);
    assert.match(html, /&lt;img/);
    assert.doesNotMatch(html, /<img|https:|<a /);
  }
});

test('Rust terminal lookup requires a unique full UID and instance, never a name fallback', () => {
  const T = {list: [{name: 'sessiondock-codex-abcdefgh', uid: 'codex:other', instance_id: 'instance'}], pending: [], uid: 'codex:wanted', name: 'sessiondock-codex-abcdefgh'};
  const context = contextWithCapabilities(disabled, {T, sessionTermMeta: () => ({sid: 'abcdefgh-more', source: 'codex'})});
  const linked = loadFunction(context, 'linkedTermSession', read('term.js'));
  assert.equal(linked('codex:wanted', {followReplacement: true}), null);
  assert.equal(linked('tmux:sessiondock-codex-abcdefgh'), null);
  T.list[0].uid = 'codex:wanted';
  assert.equal(linked('codex:wanted').name, T.name);
  T.list.push({...T.list[0], name: 'duplicate'});
  assert.equal(linked('codex:wanted'), null);
  T.list.pop();
  delete T.list[0].instance_id;
  assert.equal(linked('codex:wanted'), null);
  const python = contextWithCapabilities(undefined, {T, sessionTermMeta: context.sessionTermMeta});
  assert.equal(loadFunction(python, 'linkedTermSession', read('term.js'))('tmux:sessiondock-codex-abcdefgh').name, T.name);
});

test('terminal ownership force retry retains the exact captured binding', async () => {
  const calls = [];
  const prompts = [];
  const ConsoleUI = {errors: new Map([['codex:uid', '上次失败']]), busy: new Set()};
  const context = contextWithCapabilities(disabled, {T: {}, TERM_PAGE_ID: 'page', TERM_CLAIM_TIMEOUT_MS: 5000,
    ConsoleUI, renderTakeoverBtn: () => {},
    confirm: message => { prompts.push(message); return true; },
    post: async (_path, body) => {calls.push(body); return calls.length === 1 ? {conflict: true, owner: {ip: '10.66.66.1', label: ''}} : {token: 'lease'};}});
  loadFunction(context, 'describeTermTaker', read('term.js'));
  const claim = loadFunction(context, 'claimTermOwnership', read('term.js'));
  assert.equal(await claim('name', 'codex:uid', {uid: 'codex:uid', instance_id: 'captured-instance'}), 'lease');
  assert.equal(ConsoleUI.errors.has('codex:uid'), false, 'a granted lease clears the remembered failure');
  for (const body of calls) {
    assert.equal(body.uid, 'codex:uid');
    assert.equal(body.instance_id, 'captured-instance');
  }
  assert.equal(calls[1].force, true);
  // Without a label, and without the server marking the holder elsewhere, the
  // prompt names no address (an older node's hub tunnel address says nothing).
  assert.equal(prompts[0], '该终端正由另一页面控制。\n\n是否抢占终端？');
});

test('composer takeover claims the captured terminal directly without a second prompt', async () => {
  const calls = [];
  const context = contextWithCapabilities(disabled, {T: {}, TERM_PAGE_ID: 'page', TERM_CLAIM_TIMEOUT_MS: 5000,
    ConsoleUI: {errors: new Map(), busy: new Set()}, renderTakeoverBtn: () => {},
    confirm: () => assert.fail('the explicit takeover button already authorizes the claim'),
    post: async (_path, body) => {calls.push(body);return {token: 'new-lease'};}});
  const claim = loadFunction(context, 'claimTermOwnership', read('term.js'));
  assert.equal(await claim('name', 'codex:uid',
    {uid: 'codex:uid', instance_id: 'captured-instance'}, false, true), 'new-lease');
  assert.deepEqual(JSON.parse(JSON.stringify(calls)), [{name: 'name', page: 'page', force: true,
    uid: 'codex:uid', instance_id: 'captured-instance'}]);
});

test('an automatic pty restore never asks to take over a held terminal', async () => {
  const calls = [];
  const context = contextWithCapabilities(disabled, {T: {}, TERM_PAGE_ID: 'page', TERM_CLAIM_TIMEOUT_MS: 5000,
    auditTermPane: () => {},
    confirm: () => assert.fail('entering the conversation view must not prompt'),
    alert: () => assert.fail('entering the conversation view must not alert'),
    post: async (_path, body) => {calls.push(body); return {conflict: true, owner: {ip: '203.0.113.7', label: 'iPhone · Safari'}, same_address: false};}});
  loadFunction(context, 'describeTermTaker', read('term.js'));
  const claim = loadFunction(context, 'claimTermOwnership', read('term.js'));
  assert.equal(await claim('name', 'codex:uid', {uid: 'codex:uid', instance_id: 'i'}, true), null);
  assert.equal(calls.length, 1, 'no force claim follows a silent decline');
  assert.equal(calls[0].force, undefined);
});

test('terminal claim POST deadline covers headers and body without retrying input', async () => {
  for (const phase of ['headers', 'body', 'success', 'input']) {
    const timers = new Map(), audits = [];
    let requests = 0, signal;
    const context = vm.createContext({BUILD_ID: 'fixture', TERM_PAGE_ID: 'fixture-page',
      AbortController, performance, appUrl: path => path,
      navigator: {onLine: true}, document: {visibilityState: 'visible'},
      setTimeout: (fn, ms) => { timers.set(1, {fn, ms}); return 1; },
      clearTimeout: id => timers.delete(id),
      browserAuditEvent: (...args) => audits.push(args),
      fetch: async (_url, options) => {
        requests++;
        signal = options.signal;
        const stalled = () => new Promise((_resolve, reject) => {
          if (signal.aborted) return reject(new Error('aborted'));
          signal.addEventListener('abort', () => reject(new Error('aborted')), {once: true});
        });
        if (phase === 'headers') return stalled();
        if (phase === 'body') return {status: 200, ok: true, text: stalled};
        return {status: 200, ok: true, text: async () => JSON.stringify({token: 'lease'})};
      },
    });
    const post = loadFunction(context, 'post', read('term.js'));
    const job = phase === 'input' ? post('api/term/send', {keys: ['Escape']})
      : post('api/term/claim', {name: 'fixture'}, {timeoutMs: 5000});
    if (phase === 'headers' || phase === 'body') {
      await Promise.resolve();
      assert.equal(timers.get(1).ms, 5000);
      const rejected = assert.rejects(job, error => error.name === 'TimeoutError'
        && error.message.includes('服务端可能已执行'));
      timers.get(1).fn();
      await rejected;
      assert.equal(audits.at(-1)[0], 'http.request.failed');
      assert.equal(audits.at(-1)[1].phase, phase, 'the failed phase is audited');
      assert.equal(audits.at(-1)[1].online, true);
    } else {
      assert.equal((await job).token, 'lease');
      assert.equal(signal === undefined, phase === 'input');
    }
    assert.equal(requests, 1, 'no mutation is automatically repeated');
    assert.equal(timers.size, 0, 'deadline is cleared on success and failure');
  }
});

test('claim timeout never forces ownership or attaches using an uncertain lease', async () => {
  for (const force of [false, true]) {
    for (const auto of [false, true]) {
      const calls = [], ConsoleUI = {errors: new Map()};
      const context = vm.createContext({T: {}, ConsoleUI, TERM_PAGE_ID: 'page', TERM_CLAIM_TIMEOUT_MS: 5000,
        renderTakeoverBtn: () => {},
        alert: () => assert.fail('timeout must clear the attempt without a modal'),
        confirm: () => { assert.equal(auto, false); return true; },
        auditTermPane: () => {},
        post: async (_url, body, options) => {
          calls.push({body, options});
          if (force && calls.length === 1) return {conflict: true};
          const error = new Error('fixture deadline'); error.name = 'TimeoutError'; throw error;
        },
      });
      loadFunction(context, 'describeTermTaker', read('term.js'));
      const claim = loadFunction(context, 'claimTermOwnership', read('term.js'));
      const attempt = claim('pane', 'claude:fixture', {uid: 'claude:fixture', instance_id: 'pinned'}, auto);
      // Automatic reconnect propagates transient failures to its retry owner;
      // an explicit click instead retains an actionable uncertainty message.
      if (auto && !force) await assert.rejects(attempt, {name: 'TimeoutError'});
      else assert.equal(await attempt, null);
      assert.equal(calls.length, force && !auto ? 2 : 1);
      for (const {body, options} of calls) {
        assert.equal(body.instance_id, 'pinned');
        assert.equal(options.timeoutMs, 5000);
      }
      if (!auto) assert.match(ConsoleUI.errors.get('claude:fixture'), /服务端可能已取得控制权/);
    }
  }
});

test('terminal takeover prompts describe the taker only by what is meaningful', () => {
  const context = contextWithCapabilities(disabled, {});
  const describe = loadFunction(context, 'describeTermTaker', read('term.js'));
  assert.equal(describe('', ''), '另一页面');
  assert.equal(describe('', '203.0.113.7'), '另一页面（203.0.113.7）');
  assert.equal(describe('iPhone · Safari', ''), ' iPhone · Safari ');
  assert.equal(describe('iPhone · Safari', '203.0.113.7'), ' iPhone · Safari（203.0.113.7） ');
});

test('selecting an existing sidebar row does not rebuild the list', () => {
  let renders = 0;
  const selected = {classList: {removed: [], remove(value) { this.removed.push(value); }}};
  const row = {classList: {added: [], add(value) { this.added.push(value); }}};
  const agent = {classList: {added: [], add(value) { this.added.push(value); }}};
  const side = {
    scrollTop: 180,
    querySelector(selector) {
      if (selector.includes('data-agent')) return selector.includes('ag1') ? agent : null;
      if (selector.includes('keep-me')) return row;
      return null;
    },
    querySelectorAll(selector) {
      return selector === '.item.sel' ? [selected] : [];
    },
  };
  const context = contextWithCapabilities(disabled, {
    $: sel => sel === '#side' ? side : null,
    CSS: {escape: value => encodeURIComponent(value)},
    renderSide: () => { renders++; side.scrollTop = 0; },
  });
  const paint = loadFunction(context, 'paintSidebarSelection');
  assert.equal(paint('claude:keep-me'), true);
  assert.equal(renders, 0);
  assert.equal(side.scrollTop, 180);
  assert.deepEqual(selected.classList.removed, ['sel']);
  assert.deepEqual(row.classList.added, ['sel']);

  assert.equal(paint('claude:keep-me', 'ag1'), true);
  assert.equal(renders, 0);
  assert.deepEqual(agent.classList.added, ['sel']);

  side.scrollTop = 180;
  assert.equal(paint('claude:missing'), false);
  assert.equal(renders, 1);
  assert.equal(side.scrollTop, 180, 'fallback rebuild keeps the pixel offset');
});

test('selecting an existing pending terminal does not rebuild the full sidebar', () => {
  let renders = 0;
  const selected = {classList: {removed: [], remove(value) { this.removed.push(value); }}};
  const row = {classList: {added: [], add(value) { this.added.push(value); }}};
  const document = {
    querySelector: selector => selector.includes('tmux%3Apane') ? row : null,
    querySelectorAll: selector => selector === '#side .item.sel' ? [selected] : [],
  };
  const context = contextWithCapabilities(disabled, {
    document, CSS: {escape: value => encodeURIComponent(value)}, renderSide: () => { renders++; },
  });
  const select = loadFunction(context, 'selectPendingSidebarRow', read('term.js'));
  select('tmux:pane', false);
  assert.equal(renders, 0);
  assert.deepEqual(selected.classList.removed, ['sel']);
  assert.deepEqual(row.classList.added, ['sel']);

  select('tmux:missing', false);
  select('tmux:pane', true);
  assert.equal(renders, 2, 'new or not-yet-rendered rows still rebuild the sidebar');
});

test('the audit flush drains its response body', () => {
  // An unread fetch response keeps a 2 MiB shared-memory data pipe (one fd) in
  // the renderer until GC; at one audit POST per second the renderer's 1024-fd
  // limit fills in minutes and the tab freezes in GPU code (2026-09-15).
  const flush = appSource.slice(appSource.indexOf('async function flushBrowserAudit'),
    appSource.indexOf('function flushBrowserAuditBeacon'));
  assert.match(flush, /await response\.arrayBuffer\(\)\.catch\(\(\) => \{\}\);\n\s+if \(!response\.ok\)/);
});

test('pending terminals attach before any synchronous WebGL initialization', () => {
  const context = contextWithCapabilities(disabled, {T: {uid: 'tmux:pane'}});
  const shouldUse = loadFunction(context, 'shouldUseTermWebgl', read('term.js'));
  assert.equal(shouldUse(), false);
  assert.equal(shouldUse('codex:native'), true);
});

test('Codex side-thread detection reads only the live screen footer', () => {
  const context = contextWithCapabilities(disabled);
  const detect = loadFunction(context, 'terminalViewportHasCodexSideThread', read('term.js'));
  const lines = [
    'old Side from main thread', 'old output', 'scrollback', 'main response',
    '› Ask a follow-up question', 'gpt-5.6-sol · Ready · main thread',
  ];
  const term = {
    rows: 3,
    buffer: {active: {
      baseY: 3, length: lines.length,
      getLine: row => ({translateToString: () => lines[row]}),
    }},
  };
  assert.equal(detect(term), false, 'a stale marker in scrollback is ignored');
  lines[5] = 'gpt-5.6-sol · Side from main thread · main finished';
  assert.equal(detect(term), true);
  lines[5] = 'gpt-5.6-sol · Ready · main thread';
  assert.equal(detect(term), false, 'switching back to main clears the state');
});

test('Rust remembered terminal layouts pin the full UID and instance', () => {
  const T = {name: 'pane', mode: 'normal', height: 200,
    list: [{name: 'pane', uid: 'codex:uid', instance_id: 'instance'}]};
  const rust = contextWithCapabilities(disabled, {T});
  const saved = loadFunction(rust, 'currentTermView', read('term.js'))();
  assert.equal(saved.uid, 'codex:uid');
  assert.equal(saved.instance_id, 'instance');
  const python = contextWithCapabilities(undefined, {T});
  assert.deepEqual(Object.keys(loadFunction(python, 'currentTermView', read('term.js'))()), ['mode', 'height']);
});

test('binding appearance does not upgrade the remembered pending target', () => {
  const T = {name:'pane',uid:'tmux:pane',mode:'full',height:200,
    list:[{name:'pane',uid:'codex:native',instance_id:'instance'}],
    pending:[{name:'pane',record_id:'receipt',launch_id:'launch',instance_id:'instance'}]};
  const context=contextWithCapabilities(disabled,{T,pendingUid:name=>`tmux:${name}`});
  const saved=loadFunction(context,'currentTermView',read('term.js'))();
  assert.equal(saved.uid,'tmux:pane');
  assert.equal(saved.record_id,'receipt');
  assert.equal(saved.launch_id,'launch');
  T.uid='codex:native';
  const native=context.currentTermView();
  assert.equal(native.uid,'codex:native');
  assert.equal(native.record_id,undefined);
});

test('opening the native console releases a pending console of the same host instead of refusing', async () => {
  const calls=[];
  const T={uid:'codex:native',views:new Map([['pane',{bindingUid:'tmux:pane'}]])};
  const context=contextWithCapabilities(disabled,{T,S:{sel:null,agent:null},
    rememberTermOpen:(name,open)=>calls.push(['remember',name,open]),
    disposeTermView:name=>{calls.push(['dispose',name]); throw new Error('stop after release');}});
  loadFunction(context,'termBindingServes',read('term.js'));
  const open=loadFunction(context,'openTermPane',read('term.js'));
  await assert.rejects(open('pane'),/stop after release/);
  assert.deepEqual(calls,[['remember','pane',false],['dispose','pane']]);
});

test('Rust terminal lookup follows a Codex rollback branch to the pane bound to its ancestor', () => {
  const node = 'n'.repeat(32);
  const parent = {uid: 'codex:a', source: 'codex', sid: 'sid-a', fork_parent: true, created: '2026-09-14T00:00:00Z'};
  const branch = {uid: 'codex:b', source: 'codex', sid: 'sid-b', forked_from_id: 'sid-a', created: '2026-09-15T00:00:00Z'};
  const elsewhere = {uid: `codex:${node}~b`, node_id: node, source: 'codex', sid: 'sid-b', forked_from_id: 'sid-a'};
  const S = {sessions: [parent, branch, elsewhere], live: new Set(['codex:b'])};
  const T = {list: [{name: 'pane', uid: 'codex:a', instance_id: 'instance'}], pending: [], uid: null, name: null};
  const context = contextWithCapabilities(disabled, {T, S});
  for (const name of ['forkAncestors', 'forkChildren', 'forkLeaf', 'forkLeafUid']) loadFunction(context, name);
  for (const name of ['sessionTermMeta', 'termBindingServes']) loadFunction(context, name, read('term.js'));
  const linked = loadFunction(context, 'linkedTermSession', read('term.js'));
  // The branch has no pane of its own; the parent's pane is its console.
  const same = (actual, expected) => assert.equal(JSON.stringify(actual), JSON.stringify(expected));
  same(linked('codex:b'), {name: 'pane', uid: 'codex:b'});
  // The parent's pane now writes the branch: only a follower may resolve it.
  assert.equal(linked('codex:a'), null);
  same(linked('codex:a', {followReplacement: true}), {name: 'pane', uid: 'codex:b'});
  // Another machine's branch never inherits this machine's pane.
  assert.equal(linked(`codex:${node}~b`), null);
  // Rebinding a view bound to the parent serves the branch, not the reverse.
  assert.equal(context.termBindingServes('codex:a', 'codex:b'), true);
  assert.equal(context.termBindingServes('codex:b', 'codex:a'), false);
  assert.equal(context.termBindingServes('codex:a', 'codex:a'), true);
  // A second pane claiming the same ancestor is ambiguous.
  T.list.push({name: 'twin', uid: 'codex:a', instance_id: 'other'});
  assert.equal(linked('codex:b'), null);
  T.list.pop();
  // A missing ancestor record ends the walk.
  branch.forked_from_id = 'sid-gone';
  assert.equal(linked('codex:b'), null);
});

test('a fork parent lists its own branches, running first, then newest', () => {
  const node = 'n'.repeat(32);
  const parent = {uid: 'codex:a', source: 'codex', sid: 'sid-a', fork_parent: true};
  const older = {uid: 'codex:b', source: 'codex', sid: 'sid-b', forked_from_id: 'sid-a', created: '2026-09-14T00:00:00Z'};
  const newer = {uid: 'codex:c', source: 'codex', sid: 'sid-c', forked_from_id: 'sid-a', created: '2026-09-15T00:00:00Z'};
  const elsewhere = {uid: `codex:${node}~d`, node_id: node, source: 'codex', sid: 'sid-d', forked_from_id: 'sid-a'};
  const grandchild = {uid: 'codex:e', source: 'codex', sid: 'sid-e', forked_from_id: 'sid-b'};
  const S = {sessions: [parent, older, newer, elsewhere, grandchild], live: new Set(['codex:b'])};
  const context = contextWithCapabilities(disabled, {S});
  const children = loadFunction(context, 'forkChildren');
  assert.deepEqual(children(parent).map(s => s.uid), ['codex:b', 'codex:c'], 'direct branches on this machine only');
  S.live.clear();
  assert.deepEqual(children(parent).map(s => s.uid), ['codex:c', 'codex:b'], 'newest first once nothing runs');
  assert.equal(children(grandchild).length, 0);
  assert.equal(children({uid: 'tmux:x', source: 'codex'}).length, 0, 'a pending row without a sid has no branches');
});

test('the pending stage speaks in user terms and stays silent while nothing is wrong', () => {
  const context=contextWithCapabilities(disabled, {workerStatusMessage: () => '', pendingFirstInput: new Map(),
    PENDING_RECORD_GRACE_MS: 60_000});
  const source=read('term.js');
  context.pendingRecordMissing=loadFunction(context,'pendingRecordMissing',source);
  loadFunction(context,'pendingPhase',source);
  const message=loadFunction(context,'pendingStageMessage',source);
  assert.equal(message({state:'running',running:true}), '');
  assert.equal(message({state:'running',running:true,declared_sid:'abc'}), '');
  assert.equal(message({state:'starting'}), '正在启动…');
  assert.equal(message({state:'exited'}), '会话已结束。');
  assert.equal(message({state:'failed'}), '启动失败。');
  assert.equal(message({state:'running',running:false}), '正在停止…');
  assert.equal(message({state:'uncertain'}), '暂时无法确认会话状态。');
  assert.equal(message({state:'running',running:true,binding:{state:'confirmed',method:'process',uid:'codex:full'}}), '正在打开会话…');
  // No uid, method or evidence ever reaches the page text.
  for (const info of [{state:'running',running:true,binding:{state:'uncertain',uid:'codex:full'}}])
    assert.doesNotMatch(message(info), /codex:full|证据|操作者/);
  // The missing-record line needs a message sent from this page a minute ago.
  context.pendingFirstInput.set('pane', Date.now() - 61_000);
  assert.equal(message({name:'pane',source:'codex',state:'running',running:true}),
    '会话在运行，但还没找到它的记录，终端可以继续用。');
  assert.equal(message({name:'pane',source:'shell',state:'running',running:true}), '');
  context.pendingFirstInput.set('pane', Date.now());
  assert.equal(message({name:'pane',source:'codex',state:'running',running:true}), '');
});

test('bug-report worker rows surface the manifest status and pending rows name their state', () => {
  const context=contextWithCapabilities({...disabled, backend:'rust'});
  const source=read('term.js');
  const table=/const WORKER_STATUS_TEXT = \{[\s\S]*?\n\};/.exec(source);
  assert.ok(table, 'WORKER_STATUS_TEXT table exists');
  vm.runInContext(table[0], context);
  const worker=loadFunction(context,'workerStatusMessage',source);
  assert.equal(worker({kind:'bug-report',worker_status:'failed',worker_error:'未注入'}),'未发送，输入已保留：未注入');
  assert.equal(worker({kind:'bug-report',worker_status:'submitted'}),'已发送');
  assert.equal(worker({worker_status:'failed'}),'');
  context.workerStatusMessage=worker;
  loadFunction(context,'pendingPhase',source);
  const message=loadFunction(context,'pendingStageMessage',source);
  assert.equal(message({kind:'bug-report',worker_status:'failed',worker_error:'未注入',declared_sid:'abc'}),'未发送，输入已保留：未注入');
  const label=loadFunction(context,'pendingStateLabel',source);
  assert.equal(label({record_id:'r',state:'exited'}),'已结束');
  assert.equal(label({record_id:'r',state:'running'}),'等待首条消息');
  assert.equal(label({record_id:'r',state:'running',kind:'bug-report',worker_status:'injecting'}),'正在注入缺陷报告提示词');
});

test('a stale Rust terminal view cannot reconnect to a replacement instance', async () => {
  const errors = new Map();
  const context = contextWithCapabilities(disabled, {
    T: {list: [{name: 'pane', uid: 'codex:uid', instance_id: 'new-instance'}]},
    ConsoleUI: {errors}, renderTakeoverBtn: () => {},
    claimTermOwnership: () => assert.fail('Old view must not acquire a replacement lease'),
  });
  const result = await loadFunction(context, 'attachOwnedTerm', read('term.js'))({name: 'pane', instanceId: 'old-instance'});
  assert.equal(result, false);
  assert.match(errors.get('codex:uid'), /实例关联已失效/);
});

test('a WebSocket stuck connecting is retired through the normal reconnect path', async () => {
  let callback, timer = 0;
  const calls = [], errors = new Map();
  const ws = {readyState: 1, close: () => calls.push('close')};
  const view = {name: 'pane', ws, connectTimer: null,
    term: {write: text => calls.push(['write', text])}};
  const T = {name: 'pane', ws, views: new Map([['pane', view]])};
  const context = contextWithCapabilities(disabled, {
    TERM_CONNECT_TIMEOUT_MS: 15000, T, ConsoleUI: {errors},
    setTimeout: (fn, delay) => { callback = fn; calls.push(['timer', delay]); return ++timer; },
    clearTimeout: id => calls.push(['clear', id]), renderTakeoverBtn: () => calls.push('render'),
    browserAuditEvent: (...args) => calls.push(['audit', ...args]),
    pollLive: async force => calls.push(['poll', force]),
    scheduleTermReconnect: target => calls.push(['reconnect', target]),
  });
  context.cancelTermConnectTimeout = loadFunction(context, 'cancelTermConnectTimeout', read('term.js'));
  const arm = loadFunction(context, 'armTermConnectTimeout', read('term.js'));

  arm(view, ws, 'tmux:uid', 'connection');
  assert.deepEqual(calls.slice(0, 2), [['clear', null], ['timer', 15000]]);
  callback();
  assert.equal(view.ws, ws, 'an already-open socket is never expired');
  assert.equal(calls.includes('close'), false);

  ws.readyState = 0;
  arm(view, ws, 'tmux:uid', 'connection');
  callback();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(view.connectTimer, null);
  assert.equal(view.ws, null);
  assert.equal(T.ws, null);
  assert.match(errors.get('tmux:uid'), /建立超时.*自动重试/);
  assert.equal(calls.filter(call => call === 'close').length, 1);
  assert.ok(calls.some(call => Array.isArray(call) && call[0] === 'write' && /建立超时/.test(call[1])));
  assert.ok(calls.some(call => Array.isArray(call) && call[0] === 'audit'
    && call[1] === 'terminal.connect_timeout' && call[2].timeout_ms === 15000
    && call[4].uid === 'tmux:uid' && call[4].connectionId === 'connection'));
  assert.ok(calls.some(call => Array.isArray(call) && call[0] === 'poll' && call[1] === true));
  assert.ok(calls.some(call => Array.isArray(call) && call[0] === 'reconnect' && call[1] === view));
});

test('manual terminal capability does not enable the reliable-send composer', async () => {
  const messages = [];
  const context = contextWithCapabilities({...disabled, terminal: true}, {
    alert: text => messages.push(text),
    $: () => assert.fail('Disabled send must not read or consume a draft'),
  });
  context.conversationSendEnabled=()=>false;
  await loadFunction(context, 'submitComposer', read('term.js'))();
  assert.match(messages[0], /尚未启用会话发送/);
});

test('only an explicit Rust host exit retires reconnect without consuming a draft', () => {
  const errors = new Map(), ended = new Map(), writes = [], layouts = [];
  const context = contextWithCapabilities(disabled, {
    T: {ended}, ConsoleUI: {errors},
    cancelTermReconnect: () => {}, renderTakeoverBtn: () => {},
    rememberTermOpen: (...args) => layouts.push(args),
  });
  const exit = loadFunction(context, 'recordHostExit', read('term.js'));
  const view = {name: 'pane', instanceId: 'instance', term: {write: text => writes.push(text)}};
  assert.equal(exit(view, 'codex:uid', {code: 1011, reason: 'host stream closed without exit marker'}), false);
  assert.equal(ended.size, 0);
  assert.equal(exit(view, 'codex:uid', {code: 1011, reason: 'host output incomplete: PTY drain timeout'}), true);
  assert.equal(view.ended, true);
  assert.equal(view.revoked, true);
  assert.equal(ended.get('codex:uid').instanceId, 'instance');
  assert.match(errors.get('codex:uid'), /PTY drain timeout/);
  assert.match(writes[0], /输出不完整/);
  assert.deepEqual(layouts, [['pane', false]]);
  const python = contextWithCapabilities(undefined);
  assert.equal(loadFunction(python, 'recordHostExit', read('term.js'))(null, null, {code: 1000, reason: 'host exited'}), false);
});

test('Rust managed terminal polling is independent of global live and waits for the current read', async () => {
  let resolve, calls = 0;
  const timers = [];
  const context = contextWithCapabilities({...disabled, terminal: true, live: false}, {
    loadTermList: () => { calls++; return new Promise(done => { resolve = done; }); },
    setTimeout: (fn, delay) => timers.push(delay),
  });
  const poll = loadFunction(context, 'pollRustTermList', read('term.js'));
  const task = poll();
  assert.equal(calls, 1);
  assert.deepEqual(timers, []);
  resolve();
  await task;
  assert.deepEqual(timers, [3000]);
  context.document.hidden = true;
  await poll();
  assert.equal(calls, 1);
  assert.deepEqual(timers, [3000, 3000]);
  for (const capabilities of [undefined, {...disabled, terminal: false}, {...disabled, terminal: true, live: true}]) {
    const excluded = contextWithCapabilities(capabilities, {
      loadTermList: () => assert.fail('unexpected terminal list request'),
      setTimeout: () => assert.fail('unexpected independent poll'),
    });
    await loadFunction(excluded, 'pollRustTermList', read('term.js'))();
  }
});

test('Rust pending rows require launch identity and never run native resolution or draft cleanup', async () => {
  const row={name:'pending-host',record_id:'receipt',launch_id:'launch',instance_id:'instance',stale:false};
  const context=contextWithCapabilities(disabled,{
    T:{list:[],pending:[row]}, S:{sel:'tmux:pending-host'}, pendingUid:name=>`tmux:${name}`,
    $:()=>null, fetch:()=>assert.fail('pending must not guess native association'),
    setTimeout:()=>assert.fail('pending must use the shared list poll'),
    discardAbandonedNewSession:()=>assert.fail('pending must retain receipt and draft'),
  });
  const linked=loadFunction(context,'linkedTermSession',read('term.js'));
  assert.equal(linked('tmux:pending-host').name,'pending-host');
  assert.equal(linked('codex:pending-host'),null);
  row.stale=true;
  assert.equal(linked('tmux:pending-host'),null);
  await loadFunction(context,'resolveNewSession',read('term.js'))(row);
});

test('Hub pending lifecycle writes retain their explicit machine', async () => {
  const node='a'.repeat(32);
  const row={name:`${node}~pending-host`,node_id:node,record_id:'receipt',
    launch_id:'launch',instance_id:'instance',running:true,stale:false};
  const calls=[];
  const context=contextWithCapabilities(disabled,{
    HUB_MODE:true,T:{pending:[row]},post:async(path,body)=>{calls.push([path,body]);return {ok:true,running:false};},
    discardAbandonedNewSession:()=>calls.push(['discarded']),confirm:()=>true,
    loadTermList:async()=>{},pendingUid:name=>`tmux:${name}`,S:{sel:''},$:()=>null,alert:assert.fail,
    pendingTitle:()=>'SSH',
  });
  await loadFunction(context,'stopPendingSession',read('term.js'))(row,null);
  assert.deepEqual(JSON.parse(JSON.stringify(calls.shift())),[
    'api/term/kill',{record_id:'receipt',instance_id:'instance',_node:node},
  ]);
  row.running=true;
  await loadFunction(context,'discardPendingSession',read('term.js'))(row);
  assert.deepEqual(JSON.parse(JSON.stringify(calls)),[
    ['api/term/kill',{record_id:'receipt',instance_id:'instance',_node:node}],
    ['api/term/discard',{record_id:'receipt',instance_id:'instance',_node:node}],
    ['discarded'],
  ]);
});

test('confirmed pending binding follows native history after its host record exits', async () => {
  const row={name:'node~pending-host',record_id:'receipt',launch_id:'launch',instance_id:'instance',
    stale:true,binding:{state:'confirmed',source:'codex',sid:'native-sid',uid:'codex:node~native'}};
  const calls=[];
  const context=contextWithCapabilities(disabled,{
    T:{list:[],pending:[row],views:new Map(),openViews:new Map(),pendingModes:new Map()},
    S:{sel:'tmux:node~pending-host',agent:null,sessions:[{uid:'codex:node~native',source:'codex',sid:'native-sid'}]},
    pendingUid:name=>`tmux:${name}`, migrateComposerDraft:(from,to)=>calls.push(['draft',from,to]),
    openSession:async uid=>{calls.push(['open',uid]);context.S.sel=uid;},
    paintLive:()=>calls.push(['paint']), openTermPane:()=>assert.fail('an exited host has no terminal to reopen'),
    $:selector=>selector==='#termpane'?{classList:{contains:()=>false}}:null,
  });
  await loadFunction(context,'resolveNewSession',read('term.js'))(row);
  assert.deepEqual(calls,[
    ['draft','tmux:node~pending-host','codex:node~native'],
    ['open','codex:node~native'],
    ['paint'],
  ]);
});

test('private attachment uploads keep the exact session UID and stable upload ID', async () => {
  let request;
  class Xhr {
    upload={};status=200;responseText='{"ok":true,"upload_id":"file-id"}';
    open(method,url){request={method,url:String(url)};}
    setRequestHeader(){}
    send(body){request.body=body;this.onload();}
  }
  const context=contextWithCapabilities(disabled, {
    URL, Blob, XMLHttpRequest:Xhr,COMPOSER_MAX_FILE_BYTES:512*1024*1024,
    appUrl:path=>`http://sessiondock.test/${path}`,renderComposerItems:()=>{},
    persistComposerDraft:async()=>true,composerDraftOwner:uid=>uid,
  });
  const file=Object.assign(new Blob(['attachment'],{type:'text/plain'}),{name:'新会话附件.txt'});
  await loadFunction(context,'uploadComposerAttachment',read('term.js'))(
    {id:'file-id',file,status:'',uploaded:null},'tmux:node~pending-host');
  const query=new URL(request.url).searchParams;
  assert.equal(new URL(request.url).pathname,'/api/session/conversation/attachment');
  assert.equal(query.get('uid'),'tmux:node~pending-host');
  assert.equal(query.get('id'),'file-id');
  assert.equal(query.get('name'),'新会话附件.txt');
  assert.equal(request.method,'POST');assert.equal(request.body,file);
});

test('a card restored from the server previews the staged bytes once', async () => {
  const requests = [];
  class TestURL extends URL {
    static createObjectURL() { return 'blob:staged'; }
    static revokeObjectURL() {}
  }
  const restored = {id: 'a1', kind: 'image', preview: '',
    file: {name: 'shot.png', size: 70}, uploaded: {upload_id: 'a1', name: 'shot.png', size: 70}};
  const uid = 'tmux:node~host';
  let renders = 0;
  const context = contextWithCapabilities(disabled, {
    URL: TestURL, Blob, COMPOSER_PREVIEW_MAX_BYTES: 32 * 1024 * 1024,
    appUrl: path => `http://sessiondock.test/${path}`,
    composerDrafts: new Map([[uid, {attachments: [restored]}]]),
    composerDraftOwner: value => value,
    fetch: async url => {
      requests.push(String(url));
      return {ok: true, status: 200, blob: async () => new Blob(['png'])};
    },
  });
  const preview = loadFunction(context, 'loadStagedComposerPreview', read('term.js'));
  preview(restored, uid, () => renders++);
  for (let tick = 0; tick < 20 && !restored.preview; tick++) await new Promise(done => setTimeout(done, 0));
  assert.equal(requests.length, 1);
  const url = new URL(requests[0]);
  assert.equal(url.pathname, '/api/session/conversation/attachment');
  assert.equal(url.searchParams.get('uid'), uid);
  assert.equal(url.searchParams.get('id'), 'a1');
  assert.equal(restored.preview, 'blob:staged');
  assert.equal(renders, 1);
  // The bytes are fetched once per card, and never for one that holds the File.
  preview(restored, uid, () => renders++);
  const local = {id: 'a2', kind: 'image', preview: 'blob:local',
    file: new Blob(['x']), uploaded: {upload_id: 'a2'}};
  preview(local, uid, () => renders++);
  const huge = {id: 'a3', kind: 'image', preview: '',
    file: {name: 'huge.png', size: 64 * 1024 * 1024}, uploaded: {upload_id: 'a3'}};
  preview(huge, uid, () => renders++);
  const unstaged = {id: 'a4', kind: 'image', preview: '', file: {name: 'x.png', size: 10}, uploaded: null};
  preview(unstaged, uid, () => renders++);
  assert.equal(requests.length, 1);
  assert.equal(renders, 1);
});

test('vanished staged bytes stop retrying, a failed read tries again later', async () => {
  const statuses = [404, 503];
  const requests = [];
  class TestURL extends URL {
    static createObjectURL() { return 'blob:staged'; }
    static revokeObjectURL() {}
  }
  const uid = 'tmux:node~host';
  const cards = [{id: 'gone', kind: 'image', preview: '', file: {name: 'a.png', size: 9}, uploaded: {upload_id: 'gone'}},
    {id: 'flaky', kind: 'image', preview: '', file: {name: 'b.png', size: 9}, uploaded: {upload_id: 'flaky'}}];
  const context = contextWithCapabilities(disabled, {
    URL: TestURL, Blob, COMPOSER_PREVIEW_MAX_BYTES: 32 * 1024 * 1024,
    appUrl: path => `http://sessiondock.test/${path}`,
    composerDrafts: new Map([[uid, {attachments: cards}]]),
    composerDraftOwner: value => value,
    fetch: async url => {
      requests.push(String(url));
      return {ok: false, status: statuses.shift(), blob: async () => new Blob([])};
    },
  });
  const preview = loadFunction(context, 'loadStagedComposerPreview', read('term.js'));
  for (const card of cards) {
    preview(card, uid, () => {});
    for (let tick = 0; tick < 20 && !card.previewRetryAt; tick++) await new Promise(done => setTimeout(done, 0));
  }
  assert.equal(requests.length, 2);
  assert.equal(cards[0].previewRetryAt, Infinity);
  assert.ok(cards[1].previewRetryAt > Date.now() && cards[1].previewRetryAt < Date.now() + 120000);
  // Neither card renders a broken image, and neither hammers the node.
  for (const card of cards) preview(card, uid, () => {});
  assert.equal(requests.length, 2);
  cards[1].previewRetryAt = 0;
  statuses.push(404);
  preview(cards[1], uid, () => {});
  for (let tick = 0; tick < 20 && requests.length < 3; tick++) await new Promise(done => setTimeout(done, 0));
  assert.equal(requests.length, 3);
});

test('explicit false gates only the declared capability and namespaces Rust storage', () => {
  const {SessionDockCapabilities: caps} = contextWithCapabilities(disabled);
  assert.equal(caps.namespace, 'sessiondock.');
  assert.equal(caps.allows('audit'), false);
  assert.equal(caps.allows('watch'), true);
  assert.ok(Object.isFrozen(caps.config));
  assert.equal(contextWithCapabilities({storage_namespace: 'isolated.'})
    .SessionDockCapabilities.namespace, 'isolated.');
});

test('malformed capability metadata fails closed for background work', () => {
  for (const input of ['{bad', 'null', '[]', '"text"']) {
    const {SessionDockCapabilities: caps} = contextWithCapabilities(input);
    assert.equal(caps.config.configuration_error, true);
    for (const key of ['audit', 'live', 'outbox', 'search', 'files']) assert.equal(caps.allows(key), false);
  }
});

test('disabled audit and live never invoke fetch, beacon, or retry timers', async () => {
  const fail = () => assert.fail('Disabled background work must not run');
  const context = contextWithCapabilities(disabled, {
    fetch: fail, setTimeout: fail, clearTimeout: fail, navigator: {sendBeacon: fail},
  });
  loadFunction(context, 'browserAuditEvent')('page.loaded');
  loadFunction(context, 'scheduleBrowserSnapshot')('render');
  await loadFunction(context, 'flushBrowserAudit')();
  loadFunction(context, 'flushBrowserAuditBeacon')();
  await loadFunction(context, 'refreshLive')();
  await loadFunction(context, 'pollLive')();
});

test('absent metadata flushes audit batches through auditPayload without keepalive', async () => {
  const calls = [];
  const queue = [{event: 'a', data: {}}, {event: 'b', data: {}}];
  const context = contextWithCapabilities(undefined, {
    browserAuditQueue: queue, browserAuditTimer: 0, browserAuditSending: false, browserAuditFailCount: 0,
    AUDIT_BATCH_BYTES: 48 * 1024, AUDIT_BATCH_COUNT: 20, auditEncoder: new TextEncoder(),
    AUDIT_PAGE_ID: 'page', BUILD_ID: 'build', S: {sel: 'codex:sel'}, appUrl: value => value,
    clearTimeout: () => {}, setTimeout: () => 1, navigator: {onLine: true},
    fetch: async (url, options) => {
      calls.push({url, options});
      return {ok: true, status: 200, arrayBuffer: async () => { calls[calls.length - 1].drained = true; return new ArrayBuffer(0); }};
    },
  });
  for (const name of ['auditEventBytes', 'spliceAuditBatch', 'capAuditQueue', 'auditPayload']) loadFunction(context, name);
  await loadFunction(context, 'flushBrowserAudit')();
  assert.equal(calls.length, 1);
  assert.equal(calls[0].drained, true, 'the response body must be consumed, or its data pipe leaks a renderer fd');
  assert.equal(calls[0].url, 'api/audit/browser');
  assert.equal(calls[0].options.keepalive, undefined);
  const body = JSON.parse(calls[0].options.body);
  assert.deepEqual(Object.keys(body), ['page_id', 'uid', '_build', 'events']);
  assert.equal(body.page_id, 'page');
  assert.equal(body.uid, 'codex:sel');
  assert.equal(body.events.length, 2);
  assert.equal(queue.length, 0);
});

test('absent metadata still runs the original audit enqueue and live request', async () => {
  const events = [];
  const urls = [];
  const context = contextWithCapabilities(undefined, {
    browserAuditQueue: events, browserAuditTimer: 0, S: {sel: null},
    setTimeout: () => 1, flushBrowserAudit: () => {},
    appUrl: value => value,
    fetch: async url => {urls.push(url); throw new Error('test network boundary');},
  });
  loadFunction(context, 'browserAuditEvent')('page.loaded');
  assert.equal(events.length, 1);
  await assert.rejects(loadFunction(context, 'refreshLive')(), /test network boundary/);
  assert.deepEqual(urls, ['api/live']);
});

test('unsupported outbox does not migrate, expire, reconcile or clear saved pending input', async () => {
  const pending = [['codex:fixture', [{id: 'receipt', text: 'keep', state: 'sending', server: true}]]];
  const fail = () => assert.fail('Pending data must remain untouched');
  const context = contextWithCapabilities(disabled, {
    store: {get: () => pending, set: fail},
    S: {queued: new Map(pending)}, fetch: fail,
  });
  assert.equal(loadFunction(context, 'loadQueuedMessages')(), pending);
  for (const name of ['syncServerOutbox', 'reconcileQueuedMessages',
    'retireSupersededClaudeMessages', 'expireQueuedMessages', 'reconcilePendingSnapshot']) {
    assert.equal(loadFunction(context, name)('codex:fixture', []), false, name);
  }
  assert.equal(await loadFunction(context, 'reconcilePendingUid')('codex:fixture'), false);
  assert.equal(await loadFunction(context, 'recoverPendingWindow')('codex:fixture', new Set(['receipt'])), false);
  assert.equal((await loadFunction(context, 'reconcileAllPendingMessages')()).length, 0);
  assert.deepEqual([...context.S.queued], pending);
});

test('unsupported outbox-only SSE packet cannot cause recovery or erase pending input', async () => {
  const context = contextWithCapabilities(disabled, {
    cache: new Map([['codex:fixture', {end: 100}]]), viewKey: value => value,
    migrationReadFailures: new Map(),
    scheduleDiffRecovery: () => assert.fail('No outbox recovery when unavailable'),
  });
  loadFunction(context, 'migrationReadPaused');
  const result = await loadFunction(context, 'applyDiff')('codex:fixture', {outbox_only: true, outbox: []});
  assert.equal(result, 0);
});

test('unsupported search reports a clear error without a backend request', async () => {
  const context = contextWithCapabilities(disabled, {fetch: () => assert.fail('No unsupported search')});
  const result = await loadFunction(context, 'fetchSearch')(new URLSearchParams({q: 'hello'}));
  assert.equal(result.ok, false);
  assert.match(result.data.error, /尚未实现全文搜索/);
});

test('unknown live status cannot switch to an empty active-only view', () => {
  const messages = [];
  const context = contextWithCapabilities(disabled, {S: {activeOnly: false}, showConsoleToast: value => messages.push(value)});
  loadFunction(context, 'selectSessionScope')(true);
  assert.equal(context.S.activeOnly, false);
  assert.match(messages[0], /运行状态未知/);
});

function fakeStorage(initial = {}) {
  const map = new Map(Object.entries(initial));
  return {
    getItem: key => map.has(key) ? map.get(key) : null,
    setItem: (key, value) => map.set(key, String(value)),
    removeItem: key => map.delete(key),
    keys: () => [...map.keys()].sort(),
  };
}

test('preferences use only the configured SessionDock namespace', () => {
  const localStorage = fakeStorage({'sessiondock.theme': '"dark"', 'sessiondock.font': '"cascadia"',
    'sessiondock.width': '300', 'sessiondock.unread': '[["claude:a",{"count":2}]]'});
  const {SessionDockCapabilities: caps} = contextWithCapabilities(disabled, {localStorage});
  assert.equal(caps.stored('theme'), '"dark"');
  assert.equal(caps.stored('width'), '300');
  assert.equal(caps.stored('unread'), '[["claude:a",{"count":2}]]');
  assert.equal(caps.stored('missing'), null);
  assert.deepEqual(localStorage.keys().filter(key => key.endsWith('missing')), []);
  assert.equal(caps.stored('font', 'sessiondock.'), '"cascadia"');
  assert.match(appSource, /const v = SessionDockCapabilities\.stored\(k, STORAGE_PREFIX\);/);
  assert.match(appSource, /set: \(k, v\) => localStorage\.setItem\(STORAGE_PREFIX \+ k, JSON\.stringify\(v\)\)/);
  assert.match(read('typography.js'), /SessionDockCapabilities\.stored\('font', prefix\)/);
  assert.match(read('nodes.js'), /SessionDockCapabilities\.stored\('nodesOff', STORAGE_PREFIX\)/);
});

test('an explicit namespace never reads another namespace', () => {
  const localStorage = fakeStorage({'sessiondock.theme': '"dark"', 'custom.theme': '"light"'});
  const custom = contextWithCapabilities({storage_namespace: 'custom.'}, {localStorage}).SessionDockCapabilities;
  assert.equal(custom.stored('theme'), '"light"');
  assert.deepEqual(localStorage.keys(), ['custom.theme', 'sessiondock.theme']);
});

test('hub pages use the configured prefix of the same path', () => {
  const localStorage = fakeStorage({'sessiondock.hub./hub/.nodesOff': '["n1"]', 'sessiondock.nodesOff': '["wrong"]'});
  const meta = {content: JSON.stringify({backend: 'rust', hub: true, storage_namespace: 'sessiondock.hub./hub/.'})};
  const context = vm.createContext({localStorage, location: {pathname: '/hub/'},
    document: {querySelector: selector => selector.includes('sessiondock-mode') ? {content: 'hub'} : meta}});
  vm.runInContext(capabilitiesSource, context);
  assert.equal(context.SessionDockCapabilities.stored('nodesOff'), '["n1"]');
  assert.equal(localStorage.getItem('sessiondock.hub./hub/.nodesOff'), '["n1"]');
});


test('the installable shell is SessionDock', () => {
  const manifest = JSON.parse(read('manifest.webmanifest'));
  assert.equal(manifest.name, 'SessionDock');
  assert.equal(manifest.short_name, 'SessionDock');
  const index = read('index.html');
  // 不注册 Service Worker：它会把同源新标签页的导航绑在已有的（可能已卡死的）
  // 页面进程里。旧注册在页面加载时注销。
  assert.doesNotMatch(index, /serviceWorker\.register\(/);
  assert.match(index, /navigator\.serviceWorker\.getRegistrations\(\)/);
  assert.match(index, /item\.unregister\(\)/);
  assert.match(index, /key\.startsWith\('sessiondock-shell-'\)/);
  assert.match(index, /<meta name="apple-mobile-web-app-title" content="SessionDock">/);
  assert.match(index, /data-app-name="SessionDock" data-storage-key="sessiondock\.pwa-install-dismissed"/);
  assert.match(index, /localStorage\.getItem\(prefix \+ 'theme'\)/);
  for (const page of ['files.html', 'file.html']) assert.match(read(page), /<title>[^<]*SessionDock/);
  assert.match(read('file.js'), /· SessionDock'/);
});

test('all pages load the optional contract before their consumers', () => {
  for (const [page, consumer] of [['index.html', 'typography.js'], ['file.html', 'file.js'], ['files.html', 'file.js']]) {
    const html = read(page);
    assert.equal(html.match(/src="capabilities\.js/g).length, 1);
    const consumerAt = html.indexOf(`src="${consumer}`);
    assert.ok(consumerAt > 0, `${page} loads ${consumer}`);
    assert.ok(html.indexOf('src="capabilities.js') < consumerAt, `${page}: capabilities.js before ${consumer}`);
  }
  assert.match(read('nodes.js'), /const STORAGE_PREFIX = SessionDockCapabilities\.namespace/);
  assert.match(read('index.html'), /id="backend-notice" hidden role="status"/);
  assert.match(appSource, /#session-active'\)\.textContent = known \? active : '\?'/);
});

test('file entry delegates to FileDock and console availability remains unchanged', () => {
  assert.match(read('file.js'), /api\/session\/resolve-files/);
  assert.match(read('file.js'), /location\.replace\(destination\)/);
  assert.match(appSource, /if \(!SessionDockCapabilities\.allows\('files'\)\) throw new Error/);
  const baseline = readFileSync(new URL('../reference/legacy-web/nodes.js', import.meta.url), 'utf8');
  const start = 'function consoleUnavailableReason';
  const rustGuard = "  // Rust: an unlinked session can only be resumed through an explicitly\n  // configured resume-capable CLI profile; otherwise no name-based guessing.\n  if (SessionDockCapabilities.config.backend === 'rust' && !linked\n      && !(SessionDockCapabilities.allows('terminal_takeover')\n        && cap?.resume_sources?.[sessionTermMeta(uid)?.source || String(uid).split(':')[0]]))\n    return '该会话没有通过完整 UID 和实例校验的运行中终端；不能按名称猜测关联。';\n";
  assert.ok(read('nodes.js').includes(rustGuard));
  const replayGuard = "  if (typeof sessionRecordingReplayable === 'function' && sessionRecordingReplayable(uid))\n    return '';\n";
  const exitGuard = "  if (SessionDockCapabilities.config.backend === 'rust' && T.ended?.has(uid)) {\n    // An exited instance leaves the button as \"接管会话\"\n    // whenever the source has a resume-capable CLI profile (the click starts\n    // a fresh `--resume`); only an unresumable source keeps the gray\n    // explanation. The exited xterm is never reclaimed automatically.\n    // A shell recording is the console itself, so it must not go gray.\n    const source = sessionTermMeta(uid)?.source || String(uid).split(':')[0];\n    const resumable = !String(uid).startsWith('tmux:') && cap?.enabled\n      && SessionDockCapabilities.allows('terminal_takeover') && !!cap?.resume_sources?.[source]\n      && !linkedTermSession(uid, {followReplacement: true});\n    if (!resumable) return T.ended.get(uid).reason;\n  }\n";
  assert.ok(read('nodes.js').includes(replayGuard));
  assert.ok(read('nodes.js').includes(exitGuard));
  const pendingGuard = "  if (SessionDockCapabilities.config.backend === 'rust') {\n    const pending = T.pending?.find(row => row.record_id && pendingUid(row.name) === uid);\n    if (pending?.stale) {\n      const phase = typeof pendingPhase === 'function' ? pendingPhase(pending) : '';\n      if (phase !== 'exited' && phase !== 'failed')\n        return pending.unavailable_reason || '创建实例尚未就绪，不能连接控制台。';\n    }\n  }\n";
  assert.ok(read('nodes.js').includes(pendingGuard));
  // A repaint while the pointer rests on the button shows the current reason,
  // including the last failed claim, instead of the reason computed before it.
  const hoverToast = "  if (button.matches(':hover') || document.activeElement === button)\n    showConsoleToast(consoleUnavailableReason(uid, agent));\n";
  const baselineHoverToast = "  if (button.matches(':hover') || document.activeElement === button) showConsoleToast(reason);\n";
  assert.ok(read('nodes.js').includes(hoverToast));
  const compatible = read('nodes.js').replace(rustGuard, '').replace(replayGuard, '').replace(exitGuard, '').replace(pendingGuard, '')
    .replace(hoverToast, baselineHoverToast);
  // Availability stays compatible; the intentionally changed click handling is
  // exercised by hub_console_availability_browser.py and recorded in reference/README.md.
  const end = 'function bindConsoleButton';
  assert.equal(compatible.slice(compatible.indexOf(start), compatible.indexOf(end)),
    baseline.slice(baseline.indexOf(start), baseline.indexOf(end)));
});

function migrationContext(extra = {}, capabilities = disabled) {
  const context = contextWithCapabilities(capabilities, {
    S: {sel: 'codex:fixture', agent: null, cursors: new Map()},
    viewKey: (uid, agent) => agent ? `${uid}::${agent}` : uid,
    cache: new Map([['codex:fixture', {end: 10, version: {head: 'old'}, msgs: ['old']}]]),
    migrationReadFailures: new Map(), migrationReadRetries: new Map(),
    migrationReadProbes: new Map(), syncingViews: new Map(),
    _es: null, _esUid: null, _esRetry: null,
    AbortController, URLSearchParams, SYNC_STALL_MS: 1000,
    setTimeout: () => 1, clearTimeout: () => {},
    renderMigrationReadFailure: () => {},
    fetchMessages: async () => {throw Object.assign(new Error('历史语义尚未实现'), {status: 501, code: 'unsupported_history'});},
    RETRY_BASE_MS: 1500, RETRY_MAX_MS: 15000, _esRetryAttempt: 0,
    ...extra,
  });
  context.closeWatch = () => {context._es?.close(); context._es = null; context._esUid = null;};
  context.retryDelay = attempt => Math.min(context.RETRY_MAX_MS, context.RETRY_BASE_MS * 2 ** Math.max(0, attempt));
  for (const name of ['migrationReadPaused', 'transientReadFailure', 'retryableReadFailure', 'reportMigrationReadFailure',
    'reportReadFailure', 'retryMigrationRead', 'pauseMigrationWatch', 'probeWatchRejection']) {
    loadFunction(context, name);
  }
  return context;
}

test('transient failures are classified like Python retries; only definitive ones pause', () => {
  const context = migrationContext();
  const transient = [new TypeError('Failed to fetch'), Object.assign(new Error('aborted'), {name: 'AbortError'}),
    {status: 503, code: 'reader_busy'}, {status: 500}, {status: 502}, {status: 429}, {status: 408}];
  for (const error of transient) {
    assert.equal(context.transientReadFailure(error), true, JSON.stringify(error));
    assert.equal(context.reportReadFailure('codex:fixture', null, error), null);
  }
  assert.equal(context.migrationReadFailures.size, 0, 'no pause for transient failures');
  for (const error of [{status: 501, code: 'unsupported_history'}, {status: 404}, {status: 400}, {status: 413}, new Error('invalid snapshot')]) {
    assert.equal(context.transientReadFailure(error), false, JSON.stringify(error));
  }
  assert.ok(context.reportReadFailure('codex:fixture', null, {status: 404, error: '会话不存在'}));
  assert.equal(context.migrationReadPaused('codex:fixture', null), true);
  // Retry control: 5xx/unknown yes (a repaired server can succeed), 4xx no.
  assert.equal(context.retryableReadFailure({status: 501}), true);
  assert.equal(context.retryableReadFailure({status: 0}), true);
  assert.equal(context.retryableReadFailure({status: 404}), false);
  assert.equal(context.retryableReadFailure({status: 400}), false);
  assert.deepEqual([0, 1, 2, 3, 4, 9].map(context.retryDelay), [1500, 3000, 6000, 12000, 15000, 15000]);
});

test('a transient sync failure keeps the view live and the stream open', async () => {
  const es = {close: () => assert.fail('transient failure must not close the stream')};
  const context = migrationContext({_es: es, _esUid: 'codex:fixture', expireQueuedMessages: () => {},
    fetchMessages: async () => {throw Object.assign(new Error('只读工作池繁忙'), {status: 503, code: 'reader_busy'});}});
  assert.equal(await loadFunction(context, 'syncSession')('codex:fixture', null), 0);
  assert.equal(context.migrationReadPaused('codex:fixture', null), false);
  assert.equal(context._es, es);
  context.fetchMessages = async () => {throw new TypeError('Failed to fetch');};
  assert.equal(await context.syncSession('codex:fixture', null), 0);
  assert.equal(context.migrationReadFailures.size, 0);
});

test('a plain stream error never pauses; a rejected stream is probed and reopened with backoff', async () => {
  const sources = [];
  const timers = [];
  class FakeEventSource {
    constructor(url) {this.url = url; this.listeners = new Map(); sources.push(this);}
    addEventListener(name, callback) {this.listeners.set(name, callback);}
    close() {this.closed = true;}
  }
  let probeStatus = 503;
  const context = migrationContext({EventSource: FakeEventSource, window: {EventSource: true},
    AUDIT_PAGE_ID: 'fixture', browserAuditEvent: () => {}, appUrl: value => value,
    setTimeout: (callback, delay) => {timers.push({callback, delay}); return timers.length;},
    fetchMessages: async () => {throw Object.assign(new Error(`probe ${probeStatus}`), {status: probeStatus});}});
  loadFunction(context, 'watchSession')('codex:fixture', null);
  const first = sources[0];
  assert.equal(context.pauseMigrationWatch(first, 'codex:fixture', null), false, 'no pause without an explicit event');
  // A stream that opened and then dropped reopens after the base delay.
  first.onopen();
  first.onerror();
  assert.equal(first.closed, true);
  assert.equal(context.migrationReadFailures.size, 0);
  assert.equal(timers.at(-1).delay, 1500);
  timers.at(-1).callback();
  assert.equal(sources.length, 2);
  // A stream rejected before opening: the 503 probe is transient, so reopen
  // with growing backoff instead of pausing.
  sources[1].onerror();
  await context.migrationReadProbes.get('codex:fixture');
  await Promise.resolve();
  assert.equal(context.migrationReadFailures.size, 0);
  assert.equal(timers.at(-1).delay, 1500);
  timers.at(-1).callback();
  sources[2].onerror();
  await context.migrationReadProbes.get('codex:fixture');
  await Promise.resolve();
  assert.equal(timers.at(-1).delay, 3000);
  timers.at(-1).callback();
  // A 501 probe is definitive: pause with the HTTP reason and stop reopening.
  probeStatus = 501;
  const reopens = () => timers.filter(timer => timer.delay !== context.SYNC_STALL_MS).length;
  const pending = reopens();
  sources[3].onerror();
  await context.migrationReadProbes.get('codex:fixture');
  await Promise.resolve();
  assert.equal(context.migrationReadFailures.get('codex:fixture').status, 501);
  assert.match(context.migrationReadFailures.get('codex:fixture').message, /probe 501/);
  assert.equal(reopens(), pending, 'no reconnect scheduled for a paused view');
  assert.equal(context._es, null);
  // Pages without the capability keep the fixed 1.5 s reopen.
  const python = migrationContext({EventSource: FakeEventSource, window: {EventSource: true},
    AUDIT_PAGE_ID: 'fixture', browserAuditEvent: () => {}, appUrl: value => value,
    setTimeout: (callback, delay) => {timers.push({callback, delay}); return timers.length;}}, undefined);
  python.SessionDockCapabilities = contextWithCapabilities().SessionDockCapabilities;
  loadFunction(python, 'watchSession')('codex:fixture', null);
  sources.at(-1).onerror();
  assert.equal(timers.at(-1).delay, 1500);
});

test('sidebar append reads retry transient failures with backoff and give up on 4xx', async () => {
  for (const [status, expected] of [[503, [1500, 3000, 6000]], [404, []]]) {
    const timers = [];
    const context = contextWithCapabilities(disabled, {
      viewKey: (uid, agent) => agent ? `${uid}::${agent}` : uid, cache: new Map(), S: {cursors: new Map()},
      sidebarSyncing: new Set(), RETRY_BASE_MS: 1500, RETRY_MAX_MS: 15000,
      setTimeout: (callback, delay) => {timers.push({callback, delay}); return timers.length;},
      fetchMessages: async () => {throw Object.assign(new Error('failed'), {status});},
      retryDelay: attempt => Math.min(15000, 1500 * 2 ** Math.max(0, attempt)),
    });
    loadFunction(context, 'transientReadFailure');
    const sync = loadFunction(context, 'syncSidebarView');
    const row = {uid: 'codex:fixture', agent: null};
    await sync(row, {end: 1, head: 'h', anchor: ''}, {end: 2, head: 'h', anchor: ''});
    while (timers.length && timers.length <= expected.length) {
      const next = timers.at(-1);
      if (timers.length > expected.length) break;
      await next.callback();
      if (timers.length === expected.length && timers.at(-1) === next) break;
    }
    assert.deepEqual(timers.map(timer => timer.delay), expected, String(status));
  }
});

test('Rust stream failure preserves snapshots and pauses only its own view', () => {
  const context = migrationContext();
  const snapshot = context.cache.get('codex:fixture');
  context.reportMigrationReadFailure('codex:fixture', null, {error: '不支持历史', status: 501});
  assert.equal(context.migrationReadPaused('codex:fixture', null), true);
  assert.equal(context.migrationReadPaused('codex:fixture', 'child'), false);
  assert.equal(context.cache.get('codex:fixture'), snapshot);
  assert.equal(context.migrationReadFailures.get('codex:fixture').status, 501);
});

test('background sync, tick and watch do no work for a paused view', async () => {
  const fail = () => assert.fail('Paused view must not create more requests');
  const context = migrationContext({fetchMessages: fail, EventSource: fail, expireQueuedMessages: () => {}});
  context.reportMigrationReadFailure('codex:fixture', null, new Error('paused'));
  assert.equal(await loadFunction(context, 'syncSession')('codex:fixture', null), 0);
  loadFunction(context, 'tickSync')();
  loadFunction(context, 'watchSession')('codex:fixture', null);
});

test('stream errors queued by an old connection cannot pause the new view', () => {
  const old = {close: () => assert.fail('Old connection should be ignored here')};
  const current = {close: () => {}};
  const context = migrationContext({_es: current, _esUid: 'codex:fixture'});
  assert.equal(context.pauseMigrationWatch(old, 'codex:fixture', null, {error: 'old failure'}), false);
  assert.equal(context.migrationReadFailures.size, 0);
  assert.equal(context._es, current);
});

test('a rejected stream is probed once; a definitive reason pauses, a transient one does not', async () => {
  let requests = 0;
  let status = 501;
  const es = {close: () => {}};
  const context = migrationContext({_es: es, _esUid: 'codex:fixture',
    fetchMessages: async () => {requests++; throw Object.assign(new Error('无法安全展示此历史'), {status});}});
  assert.equal(context.pauseMigrationWatch(es, 'codex:fixture', null), false);
  const probe = context.probeWatchRejection('codex:fixture', null);
  assert.equal(context.probeWatchRejection('codex:fixture', null), probe, 'one probe per view');
  assert.equal(await probe, true);
  assert.equal(requests, 1);
  assert.match(context.migrationReadFailures.get('codex:fixture').message, /无法安全展示/);
  assert.equal(context.migrationReadFailures.get('codex:fixture').status, 501);
  assert.equal(context._es, null);
  context.migrationReadFailures.clear();
  status = 503;
  assert.equal(await context.probeWatchRejection('codex:fixture', null), false);
  assert.equal(context.migrationReadFailures.size, 0);
});

test('an explicit failure event needs no HTTP probe', () => {
  const es = {close: () => {}};
  const context = migrationContext({_es: es, _esUid: 'codex:fixture',
    fetchMessages: () => assert.fail('The event already carries a precise reason')});
  assert.equal(context.pauseMigrationWatch(es, 'codex:fixture', null,
    {error: '原生输入在读取期间变化，请重试', status: 503, code: 'session_error'}), false);
  assert.equal(context.migrationReadFailures.size, 0,
    'a transient migration-error must leave the view live for SSE reconnect');
  assert.equal(context.pauseMigrationWatch(es, 'codex:fixture', null,
    {error: '文件不存在', status: 404, code: 'session_error'}), true);
  assert.equal(context.migrationReadProbes.size, 0);
  assert.equal(context.migrationReadFailures.get('codex:fixture').status, 404);
});

test('failed explicit retry preserves snapshot and remains retryable', async () => {
  const context = migrationContext();
  const snapshot = context.cache.get('codex:fixture');
  context.reportMigrationReadFailure('codex:fixture', null, new Error('connection lost'));
  assert.equal(await context.retryMigrationRead('codex:fixture', null), false);
  assert.equal(context.cache.get('codex:fixture'), snapshot);
  assert.equal(context.migrationReadPaused('codex:fixture', null), true);
  assert.equal(context.migrationReadFailures.get('codex:fixture').retrying, undefined);
  assert.equal(context.migrationReadRetries.size, 0);
});

test('only a successful explicit HTTP retry clears the failure and restarts watch', async () => {
  let resolveRead;
  let requests = 0;
  const events = [];
  const context = migrationContext({
    fetchMessages: () => {requests++; return new Promise(resolve => {resolveRead = resolve;});},
    cachePut: (key, value) => {context.cache.set(key, value);},
    renderSession: async () => {events.push('render');},
    watchSession: () => {events.push('watch');},
  });
  context.reportMigrationReadFailure('codex:fixture', null, new Error('paused'));
  const first = context.retryMigrationRead('codex:fixture', null);
  const second = context.retryMigrationRead('codex:fixture', null);
  await Promise.resolve();
  assert.equal(requests, 1);
  assert.equal(context.migrationReadPaused('codex:fixture', null), true);
  resolveRead({data: {meta: {uid: 'codex:fixture'}, messages: ['fresh'],
    version: {head: 'new'}, end: 20, anchor: 'new-anchor'}, bytes: 30});
  assert.equal(await first, true);
  assert.equal(await second, true);
  assert.equal(context.migrationReadPaused('codex:fixture', null), false);
  assert.deepEqual(events, ['render', 'watch']);
  assert.deepEqual(context.cache.get('codex:fixture').msgs, ['fresh']);
});

test('a retry completed after switching views must not render or replace the new watch', async () => {
  const context = migrationContext({
    fetchMessages: async () => {
      context.S.sel = 'codex:other';
      return {data: {meta: {uid: 'codex:fixture'}, messages: [], version: {head: 'new'}, end: 20}};
    },
    cachePut: (key, value) => {context.cache.set(key, value);},
    renderSession: () => assert.fail('Must not replace another view'),
    watchSession: () => assert.fail('Must not replace another stream'),
  });
  context.reportMigrationReadFailure('codex:fixture', null, new Error('paused'));
  assert.equal(await context.retryMigrationRead('codex:fixture', null), true);
  assert.equal(context.S.sel, 'codex:other');
});

test('switching views during asynchronous retry rendering cannot replace the new stream', async () => {
  const context = migrationContext({
    fetchMessages: async () => ({data: {meta: {uid: 'codex:fixture'}, messages: [], version: {head: 'new'}, end: 20}}),
    cachePut: (key, value) => {context.cache.set(key, value);},
    renderSession: async () => {context.S.sel = 'codex:other';},
    watchSession: () => assert.fail('Late render must not replace another stream'),
  });
  context.reportMigrationReadFailure('codex:fixture', null, new Error('paused'));
  assert.equal(await context.retryMigrationRead('codex:fixture', null), true);
  assert.equal(context.S.sel, 'codex:other');
});

test('Rust HTTP errors retain backend details, while Python keeps its original message', async () => {
  for (const rust of [true, false]) {
    const context = contextWithCapabilities(rust ? disabled : undefined, {
      URLSearchParams, performance: {now: () => 0}, appUrl: value => value,
      AUDIT_PAGE_ID: 'page', BUILD_ID: 'build', browserAuditEvent: () => {},
      fetch: async () => ({ok: false, status: 501,
        json: async () => {
          assert.equal(rust, true, 'Python error responses are not consumed differently');
          return {error: '无法安全展示此历史', code: 'unsupported_history'};
        }}),
    });
    await assert.rejects(loadFunction(context, 'fetchMessages')('codex:fixture'), error => {
      assert.equal(error.status, 501);
      assert.equal(error.message, rust ? '无法安全展示此历史' : 'HTTP 501');
      return true;
    });
  }
});

test('Python pages do not gain migration pausing or change their retry policy', () => {
  const context = migrationContext({}, undefined);
  // Passing undefined selects the helper default, so explicitly use the real
  // no-meta contract here to exercise the inherited default path.
  context.SessionDockCapabilities = contextWithCapabilities().SessionDockCapabilities;
  assert.equal(context.reportMigrationReadFailure('codex:fixture', null, new Error('network')), null);
  assert.equal(context.migrationReadPaused('codex:fixture', null), false);
  assert.equal(context.pauseMigrationWatch({}, 'codex:fixture', null), false);
  assert.equal(context.migrationReadFailures.size, 0);
});

test('the real watch listener passes status and ignores stale connections', () => {
  const sources = [];
  class FakeEventSource {
    constructor() {this.listeners = new Map(); sources.push(this);}
    addEventListener(name, callback) {this.listeners.set(name, callback);}
    close() {this.closed = true;}
  }
  const context = migrationContext({EventSource: FakeEventSource, window: {EventSource: true},
    AUDIT_PAGE_ID: 'fixture', browserAuditEvent: () => {}, appUrl: value => value});
  loadFunction(context, 'watchSession')('codex:fixture', null);
  const old = sources[0];
  context.watchSession('codex:fixture', null);
  old.listeners.get('migration-error')({data: '{"status":501,"error":"old"}'});
  assert.equal(context.migrationReadFailures.size, 0);
  sources[1].listeners.get('migration-error')({data: '{"status":501,"error":"unsupported","code":"unsupported_history"}'});
  assert.equal(context.migrationReadFailures.get('codex:fixture').message, 'unsupported');
  assert.equal(context.migrationReadFailures.get('codex:fixture').status, 501);
  assert.equal(sources[1].closed, true);
});

test('failure presentation inserts a text-only alert without clearing messages or console', () => {
  const inserted = [];
  const element = () => ({style: {}, append(...items) {this.children = items;}, setAttribute() {}});
  const heading = {after: notice => inserted.push(notice)};
  const detail = {dataset: {}, querySelector: () => heading};
  const context = migrationContext();
  context.document.createElement = element;
  context.$ = selector => selector === '#detail' ? detail : null;
  loadFunction(context, 'renderMigrationReadFailure');
  context.reportMigrationReadFailure('codex:fixture', null, {error: '<script>not executable</script>', status: 501});
  assert.equal(detail.dataset.migrationStale, 'true');
  assert.equal(inserted.length, 1);
  assert.match(inserted[0].children[0].textContent, /先前快照/);
  assert.match(inserted[0].children[0].textContent, /<script>not executable<\/script>/);
  assert.equal(inserted[0].children[1].disabled, false);
  assert.equal(Object.hasOwn(detail, 'innerHTML'), false);
  // A 4xx cannot be fixed by re-reading the same view: no retry control.
  context.reportMigrationReadFailure('codex:fixture', null, {error: '会话不存在', status: 404});
  assert.equal(inserted.length, 2);
  assert.equal(inserted[1].children.length, 1);
  assert.match(inserted[1].children[0].textContent, /HTTP 404/);
  // Without a snapshot the body already says 读取失败; no duplicate banner.
  context.cache.delete('codex:fixture');
  context.reportMigrationReadFailure('codex:fixture', null, {error: '会话不存在', status: 404});
  assert.equal(inserted.length, 2);
  assert.equal(detail.dataset.migrationStale, 'true');
});

test('timeline pins are capability gated and never claim a native rewind', () => {
  const python = contextWithCapabilities(undefined);
  assert.equal(loadFunction(python, 'timelinePinEnabled')(), false);
  const withoutPin = contextWithCapabilities({...disabled, metadata: true});
  assert.equal(loadFunction(withoutPin, 'timelinePinEnabled')(), false);
  const inserted = [];
  const element = () => ({style: {}, dataset: {}, append(...items) {this.children = items;}, setAttribute() {}});
  const heading = {after: notice => inserted.push(notice)};
  const detail = {querySelector: () => heading};
  const context = contextWithCapabilities({...disabled, metadata: true, timeline_pin: true}, {
    document: {querySelector: () => ({content: JSON.stringify({...disabled, metadata: true, timeline_pin: true})}), createElement: element},
    $: selector => selector === '#detail' ? detail : null,
  });
  assert.equal(loadFunction(context, 'timelinePinEnabled')(), true);
  const render = loadFunction(context, 'renderTimelinePinNotice');
  render({uid: 'claude:fixture', timeline_pin: {tip: 'a2', target: 'u3', retired: false, native_rewind: false}});
  assert.equal(inserted.length, 1);
  assert.match(inserted[0].children[0].textContent, /CLI 未回滚/);
  assert.equal(inserted[0].dataset.retired, 'false');
  assert.equal(inserted[0].children[1].textContent, '取消固定');
  render({uid: 'claude:fixture', timeline_pin: {tip: 'a2', retired: true,
    retired_reason: 'native_advanced', retired_message: '<b>CLI 未回滚</b>，已在固定点之后继续'}});
  assert.equal(inserted.length, 2);
  assert.match(inserted[1].children[0].textContent, /固定显示已失效/);
  assert.match(inserted[1].children[0].textContent, /<b>CLI 未回滚<\/b>/);
  assert.equal(inserted[1].dataset.retiredReason, 'native_advanced');
  assert.equal(inserted[1].children[1].textContent, '清除记录');
  // Subagent views and sessions without a pin render nothing.
  render({uid: 'claude:fixture', agent_id: 'agent', timeline_pin: {tip: 'a2'}});
  render({uid: 'claude:fixture'});
  assert.equal(inserted.length, 2);
});


test('SSH terminal receipts show terminal state without native binding messages', () => {
  const context = contextWithCapabilities({...disabled, terminal: true}, {workerStatusMessage: () => '',
    pendingRecordMissing: () => false});
  const source = read('term.js');
  loadFunction(context, 'pendingPhase', source);
  const label = loadFunction(context, 'pendingStateLabel', source);
  const message = loadFunction(context, 'pendingStageMessage', source);
  assert.equal(label({source: 'shell', record_id: 'r', state: 'running'}), '交互式终端');
  assert.equal(label({source: 'shell', record_id: 'r', state: 'exited'}), '已结束');
  assert.equal(message({source: 'shell', state: 'running', running: true}), '');
  // The receipt is the session: an exited SSH says whether a recording is left to replay.
  assert.equal(message({source: 'shell', state: 'exited', running: false, recording: {id: 'r'}}), '会话已结束。');
  assert.equal(message({source: 'shell', state: 'exited', running: false}), '会话已结束，没有留下录制。');
  // The exit this page observed itself wins over a list row that still says running.
  context.T = {ended: new Map([['tmux:pane', {instanceId: 'i'}]])};
  assert.equal(label({source: 'shell', name: 'pane', instance_id: 'i', record_id: 'r', state: 'running', running: true}), '已结束');
});


test('new pending sessions enter conversation mode, remembered terminal choices are restored', async () => {
  const calls = [];
  const context = contextWithCapabilities(disabled, {
    T: {openViews:new Map(), views:new Map()}, showNewSessionStage:row=>calls.push(['conversation',row.name]),
    openTermPane:async name=>calls.push(['terminal',name]), resolveNewSession:row=>calls.push(['resolve',row.name]),
  });
  const open = loadFunction(context, 'openPendingSession', read('term.js'));
  await open({name:'new',running:true,stale:false});
  assert.deepEqual(JSON.parse(JSON.stringify(calls)), [['conversation','new'],['resolve','new']]);
  calls.length = 0;
  context.T.openViews.set('remembered', {});
  await open({name:'remembered',running:true,stale:false});
  assert.deepEqual(JSON.parse(JSON.stringify(calls)), [['conversation','remembered'],['terminal','remembered'],['resolve','remembered']]);
});


test('SSH pending sessions open the PTY instead of staying on the conversation surface', async () => {
  const calls = [];
  const context = contextWithCapabilities(disabled, {
    T: {openViews:new Map(), views:new Map()}, showNewSessionStage:row=>calls.push(['conversation',row.name]),
    openTermPane:async (name, _focus, mode)=>calls.push(['terminal',name,mode ?? null]),
    resolveNewSession:row=>calls.push(['resolve',row.name]),
  });
  const open = loadFunction(context, 'openPendingSession', read('term.js'));
  await open({name:'ssh',source:'shell',running:true,stale:false});
  assert.deepEqual(JSON.parse(JSON.stringify(calls)),
    [['conversation','ssh'],['terminal','ssh','collapsed'],['resolve','ssh']]);
  calls.length = 0;
  await open({name:'ssh-stale',source:'shell',running:false,stale:true});
  assert.deepEqual(JSON.parse(JSON.stringify(calls)),
    [['conversation','ssh-stale'],['resolve','ssh-stale']]);
  calls.length = 0;
  context.T.views.set('ssh-kept', {keepOutput:true});
  await open({name:'ssh-kept',source:'shell',running:false,stale:true});
  assert.deepEqual(JSON.parse(JSON.stringify(calls)),
    [['conversation','ssh-kept'],['terminal','ssh-kept','collapsed'],['resolve','ssh-kept']]);
  calls.length = 0;
  await open({name:'ssh-rec',source:'shell',running:false,stale:true,recording:{id:'rec'}});
  assert.deepEqual(JSON.parse(JSON.stringify(calls)),
    [['conversation','ssh-rec'],['terminal','ssh-rec','full'],['resolve','ssh-rec']]);
});


test('SSH host exit keeps the PTY instead of returning to conversation', () => {
  const closed = [], noted = [];
  const context = contextWithCapabilities(disabled, {
    T: {ended: new Map(), name: 'pane', pending: [{name: 'pane', source: 'shell'}], list: []},
    ConsoleUI: {errors: new Map()}, S: {sel: 'tmux:pane'},
    cancelTermReconnect: () => {}, renderTakeoverBtn: () => {}, rememberTermOpen: () => {},
    closeTermPane: (...args) => closed.push(args), pendingUid: name => `tmux:${name}`,
    showSessionStopNotice: () => {}, notePendingEnded: name => noted.push(name),
  });
  loadFunction(context, 'sessionIsPtyOnly', read('term.js'));
  const exit = loadFunction(context, 'recordHostExit', read('term.js'));
  const view = {name: 'pane', instanceId: 'instance'};
  assert.equal(exit(view, 'tmux:pane', {code: 1000, reason: 'host exited'}), true);
  assert.equal(view.keepOutput, true);
  assert.deepEqual(closed, []);
  // The page re-derives the row's state at once instead of waiting for a list poll.
  assert.deepEqual(noted, ['pane']);
});


test('SSH host exit with a recording starts read-only replay on the same pane', () => {
  const replayed = [], closed = [];
  const context = contextWithCapabilities(disabled, {
    T: {
      ended: new Map(), name: 'pane', mode: 'collapsed',
      pending: [{name: 'pane', source: 'shell', recording: {id: 'rec'}}],
      list: [], openViews: new Map([['pane', {mode: 'collapsed'}]]),
    },
    ConsoleUI: {errors: new Map()}, S: {sel: 'tmux:pane'},
    cancelTermReconnect: () => {}, renderTakeoverBtn: () => {},
    rememberTermOpen: () => {}, rememberTermLayout: () => {}, layoutTermPane: () => {},
    attachRecordingReplay: (view, row, uid) => replayed.push([view.name, row.recording.id, uid]),
    renderTimeline: () => {},
    closeTermPane: (...args) => closed.push(args),
    pendingUid: name => `tmux:${name}`,
    showSessionStopNotice: () => {}, notePendingEnded: () => {},
  });
  loadFunction(context, 'sessionIsPtyOnly', read('term.js'));
  loadFunction(context, 'startShellRecordingReplay', read('term.js'));
  const exit = loadFunction(context, 'recordHostExit', read('term.js'));
  const view = {name: 'pane', instanceId: 'instance'};
  assert.equal(exit(view, 'tmux:pane', {code: 1000, reason: 'host exited'}), true);
  assert.equal(view.keepOutput, true);
  assert.equal(context.T.mode, 'full');
  assert.deepEqual(closed, []);
  assert.deepEqual(replayed, [['pane', 'rec', 'tmux:pane']]);
});


test('SSH conversation shows a simple composer without leaving the PTY', () => {
  const box = {classList: {hidden: true, toggle(name, on) { if (name === 'hidden') this.hidden = on; }}};
  const right = {classList: {toggle() {}}};
  const drafts = [];
  const context = contextWithCapabilities({...disabled, conversation_send: true}, {
    S: {sel: 'tmux:ssh'}, T: {pending: [{name: 'ssh', source: 'shell'}], list: []},
    conversationSendEnabled: () => true, sessionTerminalEnabled: () => true, takenOver: () => 'ssh',
    pendingUid: name => `tmux:${name}`,
    $: sel => sel === '#composer' ? box : sel === '#right' ? right : null,
    switchComposerDraft: uid => drafts.push(uid), syncComposerMode: () => {},
  });
  loadFunction(context, 'sessionIsPtyOnly', read('term.js'));
  loadFunction(context, 'sessionComposerEnded', read('term.js'));
  loadFunction(context, 'renderComposer', read('term.js'));
  context.renderComposer();
  assert.equal(box.classList.hidden, false);
  assert.deepEqual(drafts, ['tmux:ssh']);
});


test('an ended pending session hides the composer', () => {
  const box = {classList: {hidden: false, toggle(name, on) { if (name === 'hidden') this.hidden = on; }}};
  const right = {classList: {toggle() {}}};
  const drafts = [];
  const paint = (sel, pending, extra = {}) => {
    drafts.length = 0;
    box.classList.hidden = false;
    const context = contextWithCapabilities({...disabled, conversation_send: true}, {
      S: {sel}, T: {pending, list: [], ended: extra.ended || new Map(), views: extra.views || new Map()},
      conversationSendEnabled: () => true, sessionTerminalEnabled: () => true,
      takenOver: () => extra.takenOver ?? (String(sel).startsWith('tmux:') ? String(sel).slice(5) : null),
      pendingUid: name => `tmux:${name}`,
      ...(extra.pendingTmuxSessions ? {pendingTmuxSessions: extra.pendingTmuxSessions} : {}),
      $: key => key === '#composer' ? box : key === '#right' ? right : null,
      switchComposerDraft: uid => drafts.push(uid), syncComposerMode: () => {},
    });
    loadFunction(context, 'sessionIsPtyOnly', read('term.js'));
    loadFunction(context, 'sessionComposerEnded', read('term.js'));
    loadFunction(context, 'renderComposer', read('term.js'));
    context.renderComposer();
    return {hidden: box.classList.hidden, drafts: [...drafts]};
  };

  assert.equal(paint('tmux:ssh', [{name: 'ssh', source: 'shell', state: 'running'}]).hidden, false);

  let result = paint('tmux:ssh', [{name: 'ssh', source: 'shell', state: 'exited', running: false}]);
  assert.equal(result.hidden, true);
  assert.deepEqual(result.drafts, [null]);

  result = paint('tmux:claude', [{name: 'claude', source: 'claude', state: 'exited', running: false}],
    {takenOver: null});
  assert.equal(result.hidden, true);

  result = paint('tmux:codex', [{name: 'codex', source: 'codex', state: 'failed'}], {takenOver: null});
  assert.equal(result.hidden, true);

  result = paint('tmux:kept', [], {takenOver: null, pendingTmuxSessions: () => [
    {name: 'kept', source: 'claude', uid: 'tmux:kept', state: 'exited', stale: true}]});
  assert.equal(result.hidden, true);

  result = paint('tmux:live', [{name: 'live', source: 'claude', state: 'running'}], {
    takenOver: 'live', ended: new Map([['tmux:live', {instanceId: 'i', reason: 'exited'}]]),
  });
  assert.equal(result.hidden, true);
});


test('restoring empty and legacy nullable drafts keeps defaults and valid input', () => {
  const context = vm.createContext({newComposerDraft: () => ({text:'',attachments:[],quotes:[],nextAttachmentNumber:1})});
  loadFunction(context, 'ensureComposerAttachmentNumbers', read('term.js'));
  const restore=loadFunction(context, 'restoreComposerDraftRecord', read('term.js'));
  for (const record of [null, {}, {attachments:null}, {text:null,attachments:null,quotes:null}]) {
    const draft=restore(record);
    assert.equal(draft.text,'');
    assert.equal(draft.attachments.length,0);
    assert.equal(draft.quotes.length,0);
  }
  const draft=restore({text:'keep me',attachments:[{id:'image',number:1,file:{name:'image.png',size:3}}],quotes:[{text:'keep quote'}]});
  assert.equal(draft.text,'keep me');
  assert.equal(draft.attachments[0].id,'image');
  assert.equal(draft.quotes[0].text,'keep quote');
});


test('reading a cleared draft removes old submission markers, merges early keystrokes and keeps a saved page baseline', async () => {
  const draft={text:'old report',attachments:[],quotes:[],requestId:'sent',report_prompt:'old task',revision:1,editVersion:0,savedVersion:0};
  const context=vm.createContext({composerDraftOwner:uid=>uid,composerHydrations:new Map(),composerPendingSaves:new Map(),
    composerDrafts:new Map([['uid',draft]]),conversationSendEnabled:()=>true,importLegacyComposer:async()=>null,
    readServerComposerDraft:async()=>({revision:2,value:{text:'',attachments:[],quotes:[]}}),
    newComposerDraft:()=>({text:'',attachments:[],quotes:[],revision:0,editVersion:0,savedVersion:0,nextAttachmentNumber:1}),
    composerDraftRecord:(d,uid)=>({text:d.text,attachments:d.attachments,quotes:d.quotes,uid}),
    refreshComposerDraft:()=>{},syncComposerUnloadProtection:()=>{}});
  for (const name of ['ensureComposerAttachmentNumbers','restoreComposerDraftRecord','mergeEarlyComposerEdit','queueComposerSave']) loadFunction(context,name,read('term.js'));
  const hydrate=loadFunction(context,'hydrateComposerDraft',read('term.js'));
  await hydrate('uid');assert.equal(draft.text,'');assert.equal(draft.requestId,undefined);assert.equal(draft.report_prompt,undefined);
  assert.equal(draft.revision,2);
  // Typing before the first read returns: a never-saved page must adopt the
  // server revision or every later save is refused; the server text comes
  // first, the early keystrokes follow, and the merge is queued for saving.
  Object.assign(draft,{revision:0,editVersion:0,savedVersion:0});context.composerHydrations.clear();
  context.readServerComposerDraft=async()=>{draft.editVersion=1;draft.text='concurrent edit';
    return {revision:3,value:{text:'other page',attachments:[{id:'srv',number:1,file:{name:'s.png',size:1}}],quotes:[]}}};
  await hydrate('uid');
  assert.equal(draft.text,'other page\nconcurrent edit');assert.equal(draft.revision,3);
  assert.equal(draft.attachments[0].id,'srv');assert.equal(draft.editVersion,2);
  assert.equal(context.composerPendingSaves.get(draft).value.text,'other page\nconcurrent edit');
  // A page that has saved keeps its CAS baseline; the save path rebases.
  Object.assign(draft,{revision:3,editVersion:2,savedVersion:2});context.composerHydrations.clear();
  context.readServerComposerDraft=async()=>({revision:5,value:{text:'newer elsewhere',attachments:[],quotes:[]}});
  await hydrate('uid');assert.equal(draft.revision,3);assert.equal(draft.text,'other page\nconcurrent edit');
});

test('a refused save rebases onto the server revision and the editing page wins', async () => {
  const draft={text:'phone typed',attachments:[],quotes:[],revision:4,editVersion:0,savedVersion:0,session:{uid:'uid'}};
  const posts=[];
  const context=vm.createContext({composerDraftOwner:uid=>uid,composerDrafts:new Map([['uid',draft]]),
    BUG_REPORT_DRAFT_UID:'report:test',composerUid:'uid',renderSavedComposerInputs:()=>{},$:()=>({}),
    composerPendingSaves:new Map(),composerSaving:new Set(),composerSaveQueues:new Map(),composerDraftWrites:Promise.resolve(),
    conversationSendEnabled:()=>true,hydrateComposerDraft:async()=>{},refreshComposerDraft:()=>{},syncComposerUnloadProtection:()=>{},
    readServerComposerDraft:async()=>({revision:7,value:{text:'laptop text',attachments:[],quotes:[]}}),
    setTimeout:(fn)=>fn(),Promise,
    post:async(url,body)=>{posts.push(body);return body.revision===7?{ok:true,draft:{revision:8,value:body.value}}:{error:'另一页面已更新草稿',code:'draft_revision'};},
    composerDraftRecord:(d,uid)=>({text:d.text,attachments:d.attachments,quotes:d.quotes,session:{...d.session,uid}})});
  for (const name of ['queueComposerSave']) loadFunction(context,name,read('term.js'));
  const persist=loadFunction(context,'persistComposerDraft',read('term.js'));
  assert.equal(await persist('uid'),true);
  assert.deepEqual(posts.map(p=>p.revision),[4,7]);
  assert.equal(posts[1].value.text,'phone typed');
  assert.equal(draft.revision,8);assert.equal(draft.storageError,'');assert.equal(draft.savedVersion,draft.editVersion);
});

test('an idle page follows a newer server draft and an editing page does not', async () => {
  const file={bytes:'ram'};
  const draft={text:'',attachments:[{id:'a',number:1,file,preview:'blob:a',status:'uploading',uploaded:null}],quotes:[],
    revision:2,editVersion:3,savedVersion:3};
  const refreshed=[];
  let row={revision:5,value:{text:'typed on the phone',attachments:[{id:'a',number:1,kind:'image',file:{name:'a.png',size:3},uploaded:{upload_id:'a-up'}},{id:'b',number:2,file:{name:'b.txt',size:1}}],quotes:[]}};
  const context=vm.createContext({composerDraftOwner:uid=>uid,composerDrafts:new Map([['uid',draft]]),conversationSendEnabled:()=>true,
    composerSending:false,composerSaving:new Set(),composerPendingSaves:new Map(),performance:{now:()=>5000},
    readServerComposerDraft:async()=>row,refreshComposerDraft:uid=>refreshed.push(uid),syncComposerUnloadProtection:()=>{},
    newComposerDraft:()=>({text:'',attachments:[],quotes:[],revision:0,editVersion:0,savedVersion:0,nextAttachmentNumber:1}),
    URL:{revokeObjectURL:()=>{}}});
  for (const name of ['ensureComposerAttachmentNumbers','restoreComposerDraftRecord','adoptServerDraft']) loadFunction(context,name,read('term.js'));
  vm.runInContext('let composerFollowBusy=false, composerFollowedAt=0;',context);
  const follow=loadFunction(context,'followServerDraft',read('term.js'));
  assert.equal(await follow('uid',2),false); // Nothing newer reported.
  assert.equal(await follow('uid',5),true);
  assert.equal(draft.text,'typed on the phone');assert.equal(draft.revision,5);
  const kept=draft.attachments.find(a=>a.id==='a');
  assert.equal(kept.file,file);assert.equal(kept.preview,'blob:a');assert.equal(kept.status,'uploading');
  assert.equal(kept.uploaded.upload_id,'a-up');assert.equal(draft.attachments.length,2);
  assert.equal(draft.savedVersion,3);assert.deepEqual(refreshed,['uid']);
  // Unsaved local edits are never overwritten.
  draft.editVersion=4;row={revision:9,value:{text:'even newer',attachments:[],quotes:[]}};
  assert.equal(await follow('uid',9),false);assert.equal(draft.text,'typed on the phone');
});

test('a new attachment is staged on add, two at a time, and a failure keeps the File for retry', async () => {
  const draft={text:'',attachments:[],quotes:[],revision:1,editVersion:0,savedVersion:0};
  const running=[];let active=0,peak=0;
  const context=vm.createContext({composerDraftOwner:uid=>uid,composerDrafts:new Map([['uid',draft]]),conversationSendEnabled:()=>true,
    Blob,renderComposerItems:()=>{},Promise,
    uploadComposerAttachment:async(attachment)=>{active++;peak=Math.max(peak,active);
      await new Promise(resolve=>running.push(resolve));active--;
      if (attachment.id==='bad') {attachment.status='failed';throw new Error('upload unavailable');}
      attachment.status='ready';attachment.uploaded={upload_id:attachment.id+'-up',uid:'uid'};return attachment.uploaded;}});
  vm.runInContext('const COMPOSER_UPLOAD_LANES=2; const composerUploadLanes=new Map();',context);
  for (const name of ['pumpComposerUploads','stageComposerAttachment']) loadFunction(context,name,read('term.js'));
  const make=id=>({id,file:new Blob(['x']),status:'',uploaded:null});
  draft.attachments.push(make('one'),make('two'),make('bad'));
  const staged=draft.attachments.map(a=>context.stageComposerAttachment(a,'uid'));
  await new Promise(r=>setImmediate(r));
  assert.equal(peak,2);assert.equal(draft.attachments[2].status,'queued');
  running.shift()();await new Promise(r=>setImmediate(r));
  assert.equal(draft.attachments[0].uploaded.upload_id,'one-up');assert.equal(running.length,2);
  running.shift()();running.shift()();
  const results=await Promise.all(staged);
  assert.equal(results[1].upload_id,'two-up');assert.equal(results[2],null);
  assert.equal(draft.attachments[2].status,'failed');assert.ok(draft.attachments[2].file instanceof Blob);
  assert.equal(draft.attachments[2].staging,undefined);
  // Already staged bytes are not uploaded again.
  assert.equal(await context.stageComposerAttachment(draft.attachments[0],'uid'),draft.attachments[0].uploaded);
});

test('a server-owned report SEND clears a clean viewer without overwriting local edits', async () => {
  const draft={text:'old report',attachments:[{id:'a',preview:'blob:old'}],quotes:[],requestId:'sent',revision:1,editVersion:0,savedVersion:0};
  const revoked=[];
  const context=vm.createContext({composerDrafts:new Map([['uid',draft]]),composerDraftOwner:uid=>uid,
    composerSaving:new Set(),composerSending:false,
    newComposerDraft:()=>({text:'',attachments:[],quotes:[],revision:0,editVersion:0,savedVersion:0,nextAttachmentNumber:1}),
    priorComposerSubmission:async ()=>({state:'sent',draft:{revision:2,value:{text:'',attachments:[],quotes:[]}}}),
    URL:{revokeObjectURL:url=>revoked.push(url)},refreshComposerDraft:()=>{},syncComposerUnloadProtection:()=>{}});
  for (const name of ['ensureComposerAttachmentNumbers','restoreComposerDraftRecord']) loadFunction(context,name,read('term.js'));
  loadFunction(context,'adoptServerDraft',read('term.js'));
  const reconcile=loadFunction(context,'reconcileComposerSubmission',read('term.js'));
  await reconcile('uid');
  assert.equal(draft.text,'');assert.equal(draft.attachments.length,0);assert.equal(draft.requestId,undefined);
  assert.deepEqual(revoked,['blob:old']);
  Object.assign(draft,{text:'new local edit',requestId:'sent',editVersion:1,savedVersion:0});
  await reconcile('uid');assert.equal(draft.text,'new local edit');
  draft.savedVersion=1;
  context.priorComposerSubmission=async()=>{draft.editVersion++;draft.text='edited during lookup';return {state:'sent',draft:{revision:3,value:{text:'',attachments:[],quotes:[]}}}};
  await reconcile('uid');assert.equal(draft.text,'edited during lookup');
  const file={bytes:'still in RAM'};
  Object.assign(draft,{text:'later saved input',attachments:[{id:'b',file,preview:'blob:new',status:''}],requestId:'sent',savedVersion:draft.editVersion});
  context.priorComposerSubmission=async()=>({state:'sent',draft:{revision:4,value:{text:'later saved input',attachments:[{id:'b',number:2,file:{name:'new.png',size:2}}],quotes:[]}}});
  await reconcile('uid');
  assert.equal(draft.text,'later saved input');assert.equal(draft.attachments[0].file,file);
  assert.equal(draft.attachments[0].preview,'blob:new');assert.deepEqual(revoked,['blob:old']);
});


test('send buttons keep the idle label and mark aria-busy instead of growing text', () => {
  const term = read('term.js');
  const css = read('style.css');
  assert.match(term, /function setSendButtonBusy\(/);
  assert.doesNotMatch(term, /button\.textContent = '发送中/);
  assert.doesNotMatch(term, /button\.textContent = '提交中/);
  assert.doesNotMatch(term, /button\.textContent = '抓取中/);
  assert.doesNotMatch(term, /button\.textContent = `上传 /);
  assert.match(term, /setSendButtonBusy\(button, '发送中'\)/);
  assert.match(css, /@keyframes send-spin/);
  assert.match(css, /\.cbtns \.btn\.go\[aria-busy="true"\]::after/);
  const attrs = new Map();
  const button = {
    textContent: '发送',
    setAttribute(name, value) { attrs.set(name, value); },
    removeAttribute(name) { attrs.delete(name); },
  };
  const setBusy = loadFunction(vm.createContext({}), 'setSendButtonBusy', term);
  setBusy(button, '发送中');
  assert.equal(button.textContent, '发送');
  assert.equal(attrs.get('aria-busy'), 'true');
  assert.equal(attrs.get('aria-label'), '发送中');
  setBusy(button, '');
  assert.equal(button.textContent, '发送');
  assert.equal(attrs.get('aria-busy'), 'false');
  assert.equal(attrs.has('aria-label'), false);
});


test('stale build disables composer and report send', () => {
  const buttons = {
    '#csend': {disabled: false},
    '#bug-report-go': {disabled: false},
    '#cadd': {disabled: false},
    '#bug-report-add': {disabled: false},
  };
  const context = vm.createContext({
    staleBuildShown: false,
    document: {body: {classList: {add() {}}, appendChild() {}}},
    el: () => ({setAttribute() {}, innerHTML: '', appendChild() {}, type: '', title: '', onclick: null}),
    $: sel => buttons[sel] || null,
  });
  loadFunction(context, 'markStaleBuild');
  context.markStaleBuild('abc');
  assert.equal(buttons['#csend'].disabled, true);
  assert.equal(buttons['#bug-report-go'].disabled, true);
  assert.equal(buttons['#cadd'].disabled, true);
  assert.equal(buttons['#bug-report-add'].disabled, true);
});


test('post() does not surface HTML as a JSON parse error', async () => {
  const stale = [];
  const context = vm.createContext({
    BUILD_ID: 'old', TERM_PAGE_ID: 'page', staleBuildShown: false,
    navigator: {onLine: true}, document: {visibilityState: 'visible'},
    appUrl: url => url, browserAuditEvent() {}, markStaleBuild: () => stale.push(1),
    performance: {now: () => 0}, crypto: {randomUUID: () => 'id'},
    fetch: async () => ({
      status: 502, ok: false,
      text: async () => '<html><head></head><h1>502 Bad Gateway</h1>',
    }),
  });
  const post = loadFunction(context, 'post', read('term.js'));
  await assert.rejects(() => post('api/session/conversation', {uid: 'x'}), /SessionDock 请求失败（HTTP 502）/);
  context.fetch = async () => ({
    status: 409, ok: false,
    text: async () => JSON.stringify({error: '页面版本已过期，请重新加载', reload: true, build: 'new'}),
  });
  const data = await post('api/session/conversation', {uid: 'x'});
  assert.equal(data.reload, true);
  assert.equal(stale.length, 1);
});
