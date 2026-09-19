import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {test} from 'node:test';
import vm from 'node:vm';

const app = readFileSync(new URL('../legacy-web/app.js', import.meta.url), 'utf8');
const nodes = readFileSync(new URL('../legacy-web/nodes.js', import.meta.url), 'utf8');

test('Codex current thread reconnects to the original guarded pane without moving old drafts', () => {
  const term = readFileSync(new URL('../legacy-web/term.js', import.meta.url), 'utf8');
  const T = {list: [{name: 'original', uid: 'codex:old', current_uid: 'codex:new', instance_id: 'instance'}], pending: []};
  const context = ctx({T, S: {sessions: []}, SessionDockCapabilities: {config: {backend: 'rust'}},
    pendingUid: name => 'tmux:' + name, forkAncestors: () => []});
  for (const name of ['sessionTermMeta', 'termBindingServes', 'linkedTermSession', 'termRowBinding']) load(context, name, term);
  assert.equal(context.linkedTermSession('codex:new').name, 'original');
  assert.equal(context.linkedTermSession('codex:old', {followReplacement: true}), null);
  assert.equal(context.termBindingServes('codex:old', 'codex:new'), true);
  assert.equal(context.termBindingServes('codex:old', 'codex:old'), false);
  assert.equal(context.termRowBinding('original', 'codex:new').uid, 'codex:old');
  // A previously started, lock-blocked resume must not hide the real owner.
  T.list.push({name: 'blocked', uid: 'codex:new', instance_id: 'other'});
  assert.equal(context.linkedTermSession('codex:new').name, 'original');
});
function load(context, name, source = app) {
  const fn = new RegExp(`^(?:async )?function ${name}\\(`, 'm').exec(source);
  let code;
  if (fn) {
    code = source.slice(fn.index, source.indexOf('\n}\n', fn.index) + 2);
  } else {
    const arrow = new RegExp(`^const ${name} = `, 'm').exec(source);
    assert.ok(arrow, name);
    let depth = 0, i = arrow.index;
    for (; i < source.length; i++) {
      if (source[i] === '{') depth++;
      else if (source[i] === '}') depth--;
      else if (source[i] === ';' && depth === 0) break;
    }
    code = source.slice(arrow.index, i + 1);
  }
  vm.runInContext(`${code}; this[${JSON.stringify(name)}] = ${name};`, context);
  return context[name];
}
function element(tag, className, textContent = '') {
  return {tag, className, textContent, disabled: false, isConnected: true, children: [],
    setAttribute() {}, appendChild(item) { this.children.push(item); item.parentElement = this; },
    querySelector(sel) { return this.children.find(item => '.' + item.className === sel) || null; },
    remove() { this.isConnected = false; }};
}
const A = 'a'.repeat(32), B = 'b'.repeat(32), TOKEN = '/api/media/' + A;
const HUB = `/api/nodes/${A}/api/media/${B}`;
const same = (actual, expected) => assert.equal(JSON.stringify(actual), JSON.stringify(expected));
function ctx(globals = {}) {
  return vm.createContext({URL, URLSearchParams, SessionDockCapabilities: {config: {}, allows: () => false},
    HUB_MODE: false, APP_BASE: new URL('http://127.0.0.1:8080/sessiondock/'), DEBUG_RUN: '',
    selectedNodeIds: () => ['n1', 'n2'], newNodeId: () => 'nid', appUrl: x => x, el: element, ...globals});
}
function fn(name, globals = {}, source = app) { return load(ctx(globals), name, source); }

test('esc escapes &<>" only; nullish is empty; unicode and already-escaped text stay as implemented', () => {
  const esc = fn('esc');
  for (const [input, out] of [[undefined, ''], [null, ''], ['', ''], [0, '0'], [false, 'false'],
    ['&<>"\'', '&amp;&lt;&gt;&quot;\''], ['<img src=x onerror=alert(1)>', '&lt;img src=x onerror=alert(1)&gt;'],
    ['&amp;', '&amp;amp;'], ['图片 🔗 <script>', '图片 🔗 &lt;script&gt;'], [{}, '[object Object]']]) {
    assert.equal(esc(input), out);
  }
  assert.equal(esc(), '');
  assert.equal(esc('a'.repeat(10000)).length, 10000);
});

test('viewKey joins with a double colon only for a truthy agent', () => {
  const viewKey = fn('viewKey');
  for (const [uid, agent, out] of [['codex:u', undefined, 'codex:u'], ['codex:u', null, 'codex:u'],
    ['codex:u', '', 'codex:u'], ['codex:u', 0, 'codex:u'], ['codex:u', 'agent', 'codex:u::agent'],
    ['codex:u', 'a::b', 'codex:u::a::b'], [null, 'a', 'null::a'], ['', '子代理', '::子代理']]) {
    assert.equal(viewKey(uid, agent), out);
  }
});

test('fmtSize/fmtTime/shortCwd follow the current unit, calendar and ellipsis rules', () => {
  const fmtSize = fn('fmtSize'), shortCwd = fn('shortCwd');
  for (const [n, out] of [[0, '0B'], [1023, '1023B'], [1024, '1K'], [1536, '2K'], [1048575, '1024K'],
    [1048576, '1.0M'], [1572864, '1.5M'], [-5, '-5B'], [null, 'nullB'], [undefined, 'NaNM']]) {
    assert.equal(fmtSize(n), out);
  }
  const ms = new Date(2026, 8, 12, 15, 4, 0).getTime(), Real = Date;
  function Frozen(...a) { return a.length ? new Real(...a) : new Real(ms); }
  Frozen.now = () => ms; Frozen.parse = Real.parse; Frozen.UTC = Real.UTC;
  const fmtTime = fn('fmtTime', {Date: Frozen});
  for (const [input, out] of [[undefined, ''], [null, ''], [0, ''],
    [new Date(2026, 8, 12, 15, 4, 0), '今天 15:04'], [new Date(2026, 8, 11, 9, 5, 0), '昨天 09:05'],
    [new Date(2026, 0, 3, 8, 7, 0), '1-03 08:07'], [new Date(2025, 11, 1, 23, 59, 0), '2025-12-01'],
    ['nope', 'NaN-NaN-NaN']]) assert.equal(fmtTime(input), out);
  for (const [path, max, out] of [[undefined, 40, '(未知)'], ['', 40, '(未知)'], ['/home/zj/proj', 40, '~/proj'],
    ['/home/zj/a/b/c/d/e', 10, '~/a/…/d/e'], ['~/abigdirectoryname/file', 10, '~/abigdirectoryname/file'],
    ['/var/lib/very/long/path', 8, '/var/…/long/path'], ['/home/zj/图片/项目', 40, '~/图片/项目']]) {
    assert.equal(shortCwd(path, max), out);
  }
});

test('appUrl resolves against APP_BASE, injects debug_run on /api/, and hub node filters', () => {
  const url = (g, path) => fn('appUrl', g)(path);
  const base = {APP_BASE: new URL('http://127.0.0.1:8080/sessiondock/')};
  assert.equal(url(base, '/api/media/' + A), `http://127.0.0.1:8080/sessiondock/api/media/${A}`);
  assert.equal(url(base, 'api/sessions'), 'http://127.0.0.1:8080/sessiondock/api/sessions');
  assert.equal(url(base, ''), 'http://127.0.0.1:8080/sessiondock/');
  assert.equal(url(base, 'https://evil.example/x'), 'https://evil.example/x');
  assert.equal(url({...base, DEBUG_RUN: 'run_1'}, 'api/sessions'), 'http://127.0.0.1:8080/sessiondock/api/sessions?debug_run=run_1');
  assert.equal(url({...base, DEBUG_RUN: 'run_1'}, 'index.html').includes('debug_run'), false);
  assert.equal(url({...base, HUB_MODE: true}, 'api/search'), 'http://127.0.0.1:8080/sessiondock/api/search?nodes=n1%2Cn2');
  assert.equal(url({...base, HUB_MODE: true}, 'api/trash/purge'), 'http://127.0.0.1:8080/sessiondock/api/trash/purge?nodes=n1%2Cn2');
  assert.equal(url({...base, HUB_MODE: true}, 'api/search?nodes=keep'), 'http://127.0.0.1:8080/sessiondock/api/search?nodes=keep');
  assert.equal(url({...base, HUB_MODE: true}, 'api/sessions').includes('nodes='), false);
  assert.equal(url({...base, HUB_MODE: true}, 'api/term/complete-dir'), 'http://127.0.0.1:8080/sessiondock/api/term/complete-dir?node=nid');
  assert.equal(url({...base, HUB_MODE: false}, 'api/search').includes('nodes='), false);
  assert.equal(url(base, null), 'http://127.0.0.1:8080/sessiondock/null');
});

test('only exact Rust boolean flags enable history pages, lazy media and continuation', () => {
  const flags = [['historyPagesEnabled', 'history_pages'], ['lazyMediaEnabled', 'media_lazy'],
    ['mediaContinuationEnabled', 'media_continuation']];
  for (const config of [{}, {backend: 'python'}, {backend: 'python', history_pages: true, media_lazy: true, media_continuation: true},
    {backend: 'rust'}, {backend: 'rust', history_pages: 1, media_lazy: 1, media_continuation: 1},
    {backend: 'rust', history_pages: false, media_lazy: false, media_continuation: false},
    {backend: 'rust', history_pages: 'true', media_lazy: 'true', media_continuation: 'true'},
    {backend: 'rust', history_pages: true, media_lazy: true, media_continuation: true}]) {
    const c = ctx({SessionDockCapabilities: {config}});
    for (const [name, flag] of flags) {
      load(c, name);
      assert.equal(c[name](), config.backend === 'rust' && config[flag] === true, `${name} ${JSON.stringify(config)}`);
    }
  }
});

test('safeMediaSrc admits local tokens, hub paths only when not lazy, remote http(s) only when allowed', () => {
  const src = (config, extra = {}) => fn('safeMediaSrc', {
    SessionDockCapabilities: {config, allows: name => extra.allows === true || extra.allows?.[name] === true},
    HUB_MODE: extra.HUB_MODE === true, appUrl: extra.appUrl || (x => 'APP:' + x), URL});
  const rustLazy = src({backend: 'rust', media_lazy: true});
  assert.equal(rustLazy(TOKEN), 'APP:' + TOKEN);
  assert.equal(rustLazy('/' + TOKEN), '');
  for (const bad of ['', null, undefined, 'https://example.invalid/x', 'javascript:alert(1)',
    'data:image/png;base64,eA==', 'file:///etc/passwd', '//example.com/x', TOKEN + '?q=1',
    '/api/media/' + 'A'.repeat(32), HUB, '<img src=x>', 'ftp://x', 'a'.repeat(4000)]) {
    assert.equal(rustLazy(bad), '', String(bad).slice(0, 60));
  }
  const rust = src({backend: 'rust', media_lazy: false}, {HUB_MODE: true});
  assert.equal(rust(HUB), 'APP:' + HUB);
  assert.equal(rust(`/api/nodes/${A.toUpperCase()}/api/media/${B}`), '');
  assert.equal(src({backend: 'rust'}, {HUB_MODE: false})(HUB), '');
  const remote = src({backend: 'rust'}, {allows: {media_remote: true}});
  assert.equal(remote('HTTP://Example.INVALID/a'), 'http://example.invalid/a');
  assert.equal(remote('https://user:pass@evil.example/x'), 'https://user:pass@evil.example/x');
  assert.equal(remote('https://evil.example/"onclick'), 'https://evil.example/%22onclick');
  assert.equal(remote('/relative'), '');
  assert.equal(src({backend: 'python'}, {allows: true})('https://example.invalid/x'), 'https://example.invalid/x');
  assert.equal(src({})('https://example.invalid/x'), '');
  assert.equal(src({backend: 'python'}, {HUB_MODE: true, allows: true})(HUB), 'APP:' + HUB);
});

test('mediaMoreInfo accepts only positive safe integers and a hex cursor or explicit null', () => {
  const info = fn('mediaMoreInfo');
  same(info({remaining: 24, total: 40, cursor: A}), {remaining: 24, total: 40, cursor: A});
  same(info({remaining: 1, total: 1, cursor: null}), {remaining: 1, total: 1, cursor: null});
  assert.equal(info({remaining: 3, total: 3, cursor: A, extra: 1}).extra, undefined);
  for (const more of [null, undefined, '', 1, true, [24, 40, A], {remaining: 0, total: 16, cursor: A},
    {remaining: -1, total: 40, cursor: A}, {remaining: 1.5, total: 40, cursor: A},
    {remaining: 24, total: 23, cursor: A}, {remaining: 24, total: 40}, {remaining: 24, total: 40, cursor: undefined},
    {remaining: 24, total: 40, cursor: 'A'.repeat(32)}, {remaining: 24, total: 40, cursor: A.slice(1)},
    {remaining: 24, total: 40, cursor: 7}, {remaining: Number.MAX_SAFE_INTEGER + 1, total: Number.MAX_SAFE_INTEGER + 1, cursor: null}]) {
    assert.equal(info(more), null, JSON.stringify(more));
  }
  same(info({remaining: Number.MAX_SAFE_INTEGER, total: Number.MAX_SAFE_INTEGER, cursor: null}),
    {remaining: Number.MAX_SAFE_INTEGER, total: Number.MAX_SAFE_INTEGER, cursor: null});
});

test('validateHistoryPage extra edges: filled gap, 10000-message cap, media objects, rejected empties', () => {
  const validate = fn('validateHistoryPage', {HISTORY_PAGE_MAX_EVENTS: 10000});
  const msg = (text = 'ok', extra = {}) => ({role: 'user', text, ...extra});
  const ok = (n, extra = {}) => ({messages: Array.from({length: n}, () => msg()),
    page: {cursor: A, next: extra.next === undefined ? B : extra.next, start: extra.start ?? 0, end: extra.end ?? n,
      stop: extra.stop ?? n + (extra.remaining ?? 1), remaining: extra.remaining ?? 1}});
  const partial = (head = 0, omitted = 3) => ({head, omitted});
  assert.equal(validate(ok(2, {start: 1, end: 3, stop: 3, remaining: 0, next: null}), partial(1, 2), A).remaining, 0);
  assert.equal(validate(ok(200, {stop: 201, remaining: 1}), partial(0, 201), A).end, 200);
  assert.equal(validate(ok(10000, {stop: 10001, remaining: 1}), partial(0, 10001), A).end, 10000);
  assert.equal(validate({messages: [msg('图片', {media: [{src: TOKEN}]})],
    page: {cursor: A, next: null, start: 0, end: 1, stop: 1, remaining: 0}}, partial(0, 1), A).end, 1);
  assert.equal(validate({messages: [msg('x', {media: []})],
    page: {cursor: A, next: null, start: 0, end: 1, stop: 1, remaining: 0}}, partial(0, 1), A).start, 0);
  assert.equal(validate({messages: [msg('链接 🔗 <b>')],
    page: {cursor: A, next: null, start: 0, end: 1, stop: 1, remaining: 0}}, partial(0, 1), A).end, 1);
  for (const [data, part, cursor] of [
    [ok(0, {end: 0, stop: 1, remaining: 1}), partial(0, 1), A],
    [ok(10001, {end: 10001, stop: 10002}), partial(0, 10002), A],
    [ok(2), partial(0, 3), A.toUpperCase()],
    [ok(2), {head: 0.5, omitted: 3}, A],
    [ok(2), {head: 0, omitted: -1}, A],
    [ok(2, {next: A.toUpperCase()}), partial(0, 3), A],
    [null, partial(0, 3), A],
    [{messages: [msg('x', {media: 'no'})], page: {cursor: A, next: null, start: 0, end: 1, stop: 1, remaining: 0}}, partial(0, 1), A],
    [{messages: [{role: 1, text: 'x'}], page: {cursor: A, next: null, start: 0, end: 1, stop: 1, remaining: 0}}, partial(0, 1), A],
  ]) assert.throws(() => validate(data, part, cursor), /不匹配/);
});

test('validateMediaPage extra edges: 16-item cap, extra fields, start mismatch, remaining=0 more', () => {
  const c = ctx();
  load(c, 'mediaMoreInfo');
  const validate = load(c, 'validateMediaPage');
  const item = i => ({src: '/api/media/' + i.toString(16).padStart(32, '0'), extra: true});
  const more = {remaining: 24, total: 40, cursor: A};
  const data = (start, end, extra = {}) => ({media: Array.from({length: end - start}, (_, i) => item(start + i)),
    page: {cursor: extra.cursor ?? A, next: extra.next === undefined ? B : extra.next, start, end, total: extra.total ?? 40,
      remaining: extra.remaining ?? 40 - end}});
  assert.equal(validate(data(16, 32), more, A).end, 32);
  assert.equal(validate(data(24, 40, {cursor: B, next: null, remaining: 0}), {remaining: 16, total: 40, cursor: B}, B).next, null);
  assert.equal(validate(data(0, 16, {total: 16, remaining: 0, next: null}), {remaining: 16, total: 16, cursor: A}, A).end, 16);
  assert.throws(() => validate(data(16, 33, {remaining: 7}), more, A), /不匹配/);
  assert.throws(() => validate(data(16, 32), {remaining: 0, total: 40, cursor: A}, A), /不匹配/);
  assert.throws(() => validate(data(15, 31), more, A), /不匹配/);
  assert.throws(() => validate(data(16, 32, {cursor: A.toUpperCase()}), more, A.toUpperCase()), /不匹配/);
});

test('consoleUnavailableReason: empty selection, stubs, hub errors, rust-only gates, Python skip', () => {
  const reason = (over, ...args) => {
    const c = ctx({
      SessionDockCapabilities: {config: {backend: 'rust'}, allows: () => true},
      T: {listLoaded: true, listError: '', enabled: true, ended: new Map(), pending: [],
        resume_sources: {codex: true}, sources: {codex: true}},
      takeover() {}, Terminal() {}, FitAddon() {},
      ConsoleUI: {errors: new Map(), busy: new Set()},
      Nodes: {list: [], errors: new Map(), capabilities: {}},
      linkedTermSession: () => ({name: 't'}), pendingUid: n => 'tmux:' + n,
      sessionTermMeta: () => ({source: 'codex'}), SOURCES: {codex: {name: 'Codex'}}, ...over});
    load(c, 'nodeOf', nodes);
    return load(c, 'consoleUnavailableReason', nodes)(...args);
  };
  assert.match(reason({}, ''), /请先选择/);
  assert.match(reason({}, null), /请先选择/);
  assert.match(reason({}, 'codex:u', 'agent'), /子代理/);
  assert.doesNotMatch(reason({}, 'codex:u', ''), /子代理/);
  assert.match(reason({takeover: 1}, 'u'), /尚未加载完成/);
  for (const missing of [{Terminal: undefined}, {FitAddon: undefined}]) {
    assert.match(reason(missing, 'u'), /浏览器终端组件加载失败/);
  }
  assert.match(reason({ConsoleUI: {errors: new Map(), busy: new Set(['u'])}}, 'u'), /正在打开控制台/);
  assert.equal(reason({T: {listLoaded: true, listError: 'list boom', enabled: true}}, 'u'), 'list boom');
  assert.match(reason({T: {listLoaded: false, listError: '', enabled: true}}, 'u'), /正在读取控制台状态/);
  const nid = A, uid = `codex:${nid}~s`, node = {id: nid, name: 'box'};
  assert.match(reason({HUB_MODE: true}, uid), /尚未加载或已被移除/);
  assert.match(reason({HUB_MODE: true, Nodes: {list: [node], errors: new Map([['term', [{node_id: nid, error: 'down'}]]]), capabilities: {}}}, uid),
    /box 终端列表请求失败：down/);
  assert.match(reason({HUB_MODE: true, Nodes: {list: [node], errors: new Map(), capabilities: {}}}, uid), /控制台状态尚未返回/);
  const hubReason = (resume, extra = {}) => reason({HUB_MODE: true,
    T: {listLoaded: true, listError: '', enabled: true, ended: new Map(), pending: [], resume_sources: {}, sources: {codex: true}},
    Nodes: {list: [node], errors: new Map(), capabilities: {[nid]: {enabled: true, sources: {codex: true}, resume_sources: {codex: resume}}}},
    linkedTermSession: () => null, ...extra}, uid);
  assert.equal(hubReason(true), '');
  assert.match(hubReason(false), /不能按名称猜测关联/);
  assert.match(hubReason(undefined), /不能按名称猜测关联/);
  assert.match(hubReason(false, {T: {listLoaded: true, enabled: true, resume_sources: {codex: true}}}), /不能按名称猜测关联/);
  assert.equal(hubReason(true, {T: {listLoaded: true, ended: new Map([[uid, {reason: '已退出'}]]), resume_sources: {}}}), '');
  assert.equal(hubReason(false, {T: {listLoaded: true, ended: new Map([[uid, {reason: '已退出'}]]), resume_sources: {codex: true}}}), '已退出');
  assert.equal(reason({T: {listLoaded: true, enabled: true, ended: new Map([['u', {reason: '已退出'}]])}}, 'u'), '已退出');
  assert.equal(reason({SessionDockCapabilities: {config: {backend: 'python'}, allows: () => true},
    T: {listLoaded: true, enabled: true, ended: new Map([['u', {reason: '已退出'}]]), sources: {codex: true}},
    linkedTermSession: () => ({name: 't'})}, 'u'), '');
  assert.match(reason({T: {listLoaded: true, enabled: true, pending: [{record_id: 'r', name: 'p', stale: true}],
    ended: new Map()}, pendingUid: () => 'u'}, 'u'), /创建实例尚未就绪/);
  assert.equal(reason({T: {listLoaded: true, enabled: true, pending: [{record_id: 'r', name: 'p', stale: true,
    unavailable_reason: '还在准备'}], ended: new Map()}, pendingUid: () => 'u'}, 'u'), '还在准备');
  assert.match(reason({T: {listLoaded: true, enabled: false, ended: new Map(), pending: []}}, 'u'), /未返回具体原因/);
  assert.match(reason({linkedTermSession: () => null, SessionDockCapabilities: {config: {backend: 'rust'}, allows: () => false},
    T: {listLoaded: true, enabled: true, ended: new Map(), pending: [], resume_sources: {}}}, 'codex:u'), /不能按名称猜测关联/);
  assert.match(reason({SessionDockCapabilities: {config: {backend: 'python'}, allows: () => true},
    linkedTermSession: () => null, T: {listLoaded: true, enabled: true, sources: {}, ended: new Map(), pending: []}}, 'codex:u'),
    /未找到可用的 Codex 命令/);
  assert.equal(reason({ConsoleUI: {errors: new Map([['u', '上次失败']]), busy: new Set()}}, 'u', null, true), '上次失败');
  assert.equal(reason({ConsoleUI: {errors: new Map([['u', '上次失败']]), busy: new Set()}}, 'u', null, false), '');
});

test('nest tree: spawned_by nests by node/source/sid, cycles stay roots, missing fields degrade to flat', () => {
  const S = {nest: true, nestClosed: new Set(), live: new Set()};
  const context = ctx({S});
  for (const name of ['spawnKey', 'nestSpecParent', 'nestParentOf', 'nestEdges', 'nestTree', 'nestStamp', 'agentRunning', 'expandRows']) load(context, name);
  const a = {uid: 'claude:a', source: 'claude', sid: 'a', updated: '2026-09-12T00:00:00Z',
    agent_items: [{id: 'ag', type: 'Task', updated: '2026-09-12T00:30:00Z'}]};
  const b = {uid: 'claude:b', source: 'claude', sid: 'b', updated: '2026-09-12T01:00:00Z', spawned_by: {source: 'claude', sid: 'a'}};
  const c = {uid: 'codex:c', source: 'codex', sid: 'c', updated: '2026-09-12T02:00:00Z', spawned_by: {source: 'claude', sid: 'a'}};
  const other = {uid: 'codex:d', source: 'codex', sid: 'd', node_id: 'n2', updated: '2026-09-12T03:00:00Z', spawned_by: {source: 'claude', sid: 'a'}};
  const {children, nested} = context.nestTree([a, b, c, other]);
  same([...nested], ['claude:b', 'codex:c']);          // other node: the spawner is not in this list
  same(children.get('claude:a').map(s => s.uid), ['claude:b', 'codex:c']);
  assert.equal(context.nestStamp(a, children), Date.parse(c.updated));   // subtree's latest activity
  const rows = [];
  context.expandRows(a, 0, children, rows, new Set(['claude:a']));
  same(rows.map(r => [r.agent ? r.agent.id : r.s.uid, r.depth]), [['claude:a', 0], ['codex:c', 1], ['claude:b', 1], ['ag', 1]]);
  assert.equal(rows[0].kids, 3);
  // Rust without the backend fields: no spawned_by means every row is a root; active is never set so no
  // agent row is running even when the owner is live.
  const {nested: flat} = context.nestTree([{uid: 'claude:x', source: 'claude', sid: 'x'}, {uid: 'claude:y', source: 'claude', sid: 'y'}]);
  assert.equal(flat.size, 0);
  S.live.add('claude:a');
  assert.equal(context.agentRunning('claude:a', a.agent_items[0]), false);
  assert.equal(context.agentRunning('claude:a', {...a.agent_items[0], active: true}), true);
  S.live.clear();
  assert.equal(context.agentRunning('claude:a', {...a.agent_items[0], active: true}), false);
  // A cycle (a spawned by b, b spawned by a) keeps the later one as a root instead of hiding both.
  const p = {uid: 'claude:p', source: 'claude', sid: 'p', spawned_by: {source: 'claude', sid: 'q'}};
  const q = {uid: 'claude:q', source: 'claude', sid: 'q', spawned_by: {source: 'claude', sid: 'p'}};
  same([...context.nestTree([p, q]).nested], ['claude:p']);
  S.nest = false;
  assert.equal(context.nestTree([a, b, c]).nested.size, 0);
  const flatRows = [];
  context.expandRows(a, 0, new Map(), flatRows, new Set());
  same(flatRows.map(r => r.s.uid), ['claude:a']);
});

test('continued-in: the old Claude file is hidden while its continuation is listed and never nests it as a child', () => {
  const S = {nest: true, nestClosed: new Set(), live: new Set(), sessions: []};
  const context = ctx({S});
  for (const name of ['spawnKey', 'nestSpecParent', 'nestParentOf', 'nestEdges', 'nestTree', 'sessionContinued', 'hiddenForkParent', 'sessionHidden']) load(context, name);
  const old = {uid: 'claude:old', source: 'claude', sid: 'old', updated: '2026-09-12T00:00:00Z', continued_in: 'claude:new'};
  // The continuation inherits the old process's environment: the scan records the old session as its spawner.
  const fresh = {uid: 'claude:new', source: 'claude', sid: 'new', updated: '2026-09-12T01:00:00Z', spawned_by: {source: 'claude', sid: 'old'}};
  const child = {uid: 'codex:c', source: 'codex', sid: 'c', updated: '2026-09-12T02:00:00Z', spawned_by: {source: 'claude', sid: 'new'}};
  S.sessions = [old, fresh, child];
  assert.equal(context.sessionContinued(old), true);
  assert.equal(context.sessionHidden(old), true);
  assert.equal(context.sessionHidden(fresh), false);
  const {children, nested} = context.nestTree([old, fresh, child]);
  same([...nested], ['codex:c']);                        // the continuation stays a root, C hangs under it
  assert.equal(children.has('claude:old'), false);
  same(children.get('claude:new').map(s => s.uid), ['codex:c']);
  // A continued_in pointing at a row that is not listed (deleted, other node) hides nothing.
  S.sessions = [old, child];
  assert.equal(context.sessionHidden(old), false);
  // Fork parents keep their own rule.
  assert.equal(context.sessionHidden({uid: 'codex:p', fork_parent: true}), true);
  assert.equal(context.sessionHidden({uid: 'codex:p', fork_parent: true, fork_parent_visible: true}), false);
});

test('Codex rollback: the deepest branch is the leaf, live siblings first, and only a newly hidden selection follows it', async () => {
  const S = {sel: 'codex:a', agent: null, live: new Set(), sessions: []};
  const T = {uid: 'codex:a'};
  const opened = [], migrated = [], audited = [];
  const MOBILE = {matches: false}, page = new Map([['mobilePage', 'detail']]);
  const context = ctx({S, T, MOBILE, store: {get: (key, fallback) => page.has(key) ? page.get(key) : fallback},
    migrateComposerDraft: (from, to) => migrated.push([from, to]),
    browserAuditEvent: (event, data, _content, fields) => audited.push([event, data, fields]),
    openSession: async (uid, agent, options) => opened.push([uid, agent, options])});
  for (const name of ['hiddenForkParent', 'forkAncestors', 'forkChildren', 'forkLeaf', 'forkLeafUid', 'followSelectedFork']) load(context, name);
  const a = {uid: 'codex:a', source: 'codex', sid: 'sid-a', fork_parent: true, created: '2026-09-14T00:00:00Z'};
  const b = {uid: 'codex:b', source: 'codex', sid: 'sid-b', forked_from_id: 'sid-a', fork_parent: true, created: '2026-09-14T06:00:00Z'};
  const c = {uid: 'codex:c', source: 'codex', sid: 'sid-c', forked_from_id: 'sid-b', created: '2026-09-15T00:00:00Z'};
  const stale = {uid: 'codex:s', source: 'codex', sid: 'sid-s', forked_from_id: 'sid-b', created: '2026-09-15T01:00:00Z'};
  const other = {uid: 'codex:o', node_id: 'n2', source: 'codex', sid: 'sid-o', forked_from_id: 'sid-a'};
  S.sessions = [a, b, c, stale, other];
  // Newest sibling wins without liveness; a live sibling wins over a newer one.
  assert.equal(context.forkLeafUid('codex:a'), 'codex:s');
  S.live.add('codex:c');
  assert.equal(context.forkLeafUid('codex:a'), 'codex:c');
  assert.equal(context.forkLeafUid('codex:b'), 'codex:c');
  assert.equal(context.forkLeafUid('codex:c'), 'codex:c');
  assert.equal(context.forkLeafUid('codex:missing'), 'codex:missing');
  // The page on the rolled-back parent moves to the leaf, console and draft included.
  assert.equal(await context.followSelectedFork(), true);
  same(opened, [['codex:c', null, {exact: true}]]);
  same(migrated, [['codex:a', 'codex:c']]);
  assert.equal(T.uid, 'codex:c');
  assert.equal(audited[0][0], 'session.fork_followed');
  // A parent the user chose to keep listed, a subagent view and a leaf itself stay put.
  opened.length = 0;
  S.sel = 'codex:c';
  assert.equal(await context.followSelectedFork(), false);
  S.sel = 'codex:a'; a.fork_parent_visible = true;
  assert.equal(await context.followSelectedFork(), false);
  a.fork_parent_visible = false; S.agent = 'agent-1';
  assert.equal(await context.followSelectedFork(), false);
  // A phone parked on the session list is not pulled into the detail page.
  S.agent = null; MOBILE.matches = true; page.set('mobilePage', 'list');
  assert.equal(await context.followSelectedFork(), false);
  page.set('mobilePage', 'detail');
  assert.equal(await context.followSelectedFork(), true);
  same(opened, [['codex:c', null, {exact: true}]]);
});

test('unread rows carry only a count; the badge colour comes from the current state, grey once exited', () => {
  const S = {unread: new Map([['u1', 3], ['u2', {count: 2, tmux: true}], ['u3', {count: '0'}]]), live: new Set(), liveTmux: new Set()};
  const context = ctx({S});
  for (const name of ['unreadRow', 'paintItemStatus']) load(context, name);
  same(context.unreadRow('u1'), {count: 3});
  same(context.unreadRow('u2'), {count: 2});                 // a saved tmux flag is ignored
  same(context.unreadRow('u3'), {count: 0});
  same(context.unreadRow('none'), {count: 0});
  const badge = (uid, tmuxName = '') => {
    const classes = new Set(), state = {textContent: '', title: '', ariaLabel: '',
      classList: {toggle: (name, on) => on ? classes.add(name) : classes.delete(name)}};
    context.paintItemStatus({dataset: {uid, tmuxName}, querySelector: () => state});
    return {classes: [...classes].sort(), text: String(state.textContent), title: state.title};   // the DOM stringifies
  };
  same(badge('u2'), {classes: ['counted', 'idle', 'visible'], text: '2', title: '2 条新内容，会话已退出'});
  S.live.add('u2');
  same(badge('u2'), {classes: ['counted', 'visible'], text: '2', title: '2 条新内容，运行中'});
  S.liveTmux.add('u2');
  same(badge('u2'), {classes: ['counted', 'tmux', 'visible'], text: '2', title: '2 条新内容，tmux 会话运行中'});
  same(badge('none'), {classes: ['idle'], text: '', title: '会话运行中'});   // not visible: neither live nor counted
  S.live.add('none');
  same(badge('none'), {classes: ['visible'], text: '', title: '会话运行中'});
  same(badge('pending', 'sessiondock-x'), {classes: ['tmux', 'visible'], text: '', title: 'tmux 会话运行中'});
});

test('liveStatusTitle says unknown without the live capability and tmux/direct with it', () => {
  const rust = fn('liveStatusTitle', {SessionDockCapabilities: {config: {backend: 'rust'}, allows: () => false}});
  assert.equal(rust(false), '运行状态未知，尚未实现进程探测');
  assert.equal(rust(true), '运行状态未知，尚未实现进程探测');
  const python = fn('liveStatusTitle', {SessionDockCapabilities: {config: {}, allows: () => true}});
  assert.equal(python(false), '运行中');
  assert.equal(python(true), '运行于 tmux');
});

test('composer attachment paths follow the destination node, including Windows drives and UNC', () => {
  const term = readFileSync(new URL('../legacy-web/term.js', import.meta.url), 'utf8');
  const prompt = fn('buildComposerPrompt', {}, term);
  for (const path of [String.raw`C:\work\sessiondock_attachments\1\截图 a.png`,
    'D:/work/sessiondock_attachments/1/截图 a.png',
    String.raw`\\server\share\sessiondock_attachments\1\截图 a.png`,
    String.raw`\\?\C:\work\sessiondock_attachments\1\截图 a.png`,
    '//server/share/sessiondock_attachments/1/截图 a.png']) {
    for (const relative_path of ['sessiondock_attachments/1/截图 a.png',
      String.raw`sessiondock_attachments\1\截图 a.png`, String.raw`.\sessiondock_attachments\1\截图 a.png`]) {
      assert.equal(prompt('查看', [{path, relative_path}], []),
        '查看\n\n附件1: ' + String.raw`.\sessiondock_attachments\1\截图 a.png`);
    }
  }
  assert.equal(prompt('', [{path_style: 'windows', relative_path: 'sessiondock_attachments/2/a.json'}]),
    '附件1: ' + String.raw`.\sessiondock_attachments\2\a.json`);
  assert.equal(prompt('', [{path_style: 'posix', path: '/work/a', relative_path: './sessiondock_attachments/1/a b.txt'}]),
    '附件1: ./sessiondock_attachments/1/a b.txt');
  assert.equal(prompt('', [{path: '/work/a', relative_path: String.raw`dir/a\b.txt`}]),
    '附件1: ' + String.raw`./dir/a\b.txt`);
  assert.equal(prompt('', [{path: String.raw`C:\work\a b.txt`}]), '附件1: ' + String.raw`C:\work\a b.txt`);
  assert.equal(prompt('unchanged'), 'unchanged');
});

test('console output: plain chunks go straight to xterm, a DEC 2026 frame is written whole', () => {
  const term = readFileSync(new URL('../legacy-web/term.js', import.meta.url), 'utf8');
  const timers = [];
  const context = ctx({
    document: {documentElement: {dataset: {theme: 'dark'}}},
    setTimeout: (callback, ms) => { timers.push({callback, ms}); return timers.length; },
    clearTimeout: id => { if (timers[id - 1]) timers[id - 1].cleared = true; },
  });
  for (const name of ['TERM_SYNC_HOLD_MAX', 'TERM_SYNC_HOLD_MS', 'stripOscColorSets', 'terminalColorChunk',
    'termSyncFrameOpen', 'writeParsedTermOutput', 'flushTermSyncHold', 'dropTermSyncHold',
    'writeTermOutput']) {
    load(context, name, term);
  }
  const {writeTermOutput, flushTermSyncHold, dropTermSyncHold, TERM_SYNC_HOLD_MAX} = context;
  const view = () => {
    const writes = [];
    return {writes, term: {write: s => writes.push(s)}, ansiTail: '', syncHold: null, syncHoldTimer: null};
  };
  const H = '\x1b[?2026h', L = '\x1b[?2026l';

  // Keystroke echo and ordinary output never wait.
  let v = view();
  writeTermOutput(v, 'a');
  writeTermOutput(v, '\x1b[31mb\x1b[0m');
  same(v.writes, ['a', '\x1b[31mb\x1b[0m']);
  assert.equal(timers.length, 0);

  // One frame over three packets: nothing reaches xterm until ?2026l, then one write.
  v = view();
  writeTermOutput(v, H + '\x1b[2K\x1b[1A');
  writeTermOutput(v, '\x1b[2K\x1b[0Gredrawn');
  same(v.writes, []);
  assert.equal(timers.length, 1);
  writeTermOutput(v, ' tail' + L + 'after');
  same(v.writes, [H + '\x1b[2K\x1b[1A\x1b[2K\x1b[0Gredrawn tail' + L + 'after']);
  assert.equal(timers[0].cleared, true);
  writeTermOutput(v, 'x');
  same(v.writes.slice(1), ['x']);

  // A complete frame inside one packet is not held; a packet that closes one
  // frame and opens the next is held from that point.
  v = view();
  writeTermOutput(v, H + 'whole' + L);
  same(v.writes, [H + 'whole' + L]);
  writeTermOutput(v, H + 'first' + L + H + 'second');
  same(v.writes.slice(1), []);
  writeTermOutput(v, L);
  same(v.writes.slice(1), [H + 'first' + L + H + 'second' + L]);

  // ?2026h split across packets is completed by terminalColorChunk's tail
  // buffer before the frame check sees it.
  v = view();
  writeTermOutput(v, 'p\x1b[?20');
  same(v.writes, ['p']);
  writeTermOutput(v, '26h\x1b[2Kq');
  same(v.writes, ['p']);
  writeTermOutput(v, L);
  same(v.writes, ['p', H + '\x1b[2Kq' + L]);

  // Fallback: the hold timer or the size cap flushes an unterminated frame.
  v = view();
  writeTermOutput(v, H + 'stuck');
  timers.at(-1).callback();
  same(v.writes, [H + 'stuck']);
  assert.equal(v.syncHold, null);
  writeTermOutput(v, 'more');
  same(v.writes, [H + 'stuck', 'more']);
  v = view();
  writeTermOutput(v, H + 'a'.repeat(TERM_SYNC_HOLD_MAX));
  same(v.writes.map(w => w.length), [H.length + TERM_SYNC_HOLD_MAX]);

  // Reattach drops a half frame silently; close flushes it.
  v = view();
  writeTermOutput(v, H + 'dropped');
  dropTermSyncHold(v);
  same(v.writes, []);
  writeTermOutput(v, H + 'closing');
  flushTermSyncHold(v);
  same(v.writes, [H + 'closing']);
});

test('OSC 10/11/12/4 reports from xterm never go to the PTY as keystrokes', () => {
  const term = readFileSync(new URL('../legacy-web/term.js', import.meta.url), 'utf8');
  const context = ctx();
  load(context, 'stripOscColorReports', term);
  const {stripOscColorReports} = context;
  const st = s => s + '\x1b\\';
  const bel = s => s + '\x07';
  assert.equal(stripOscColorReports(st('\x1b]11;rgb:f4f4/f6f6/f8f8')), '');
  assert.equal(stripOscColorReports(bel('\x1b]10;rgb:2525/2a2a/3232')), '');
  assert.equal(stripOscColorReports(st('\x1b]12;#315f9f')), '');
  assert.equal(stripOscColorReports(st('\x1b]11;rgb:0000/0000/0000')), '');
  assert.equal(stripOscColorReports(st('\x1b]4;1;rgb:a8a8/3232/3b3b')), '');
  assert.equal(stripOscColorReports(st('\x1b]4;232;rgb:0808/0808/0808')), '');
  assert.equal(stripOscColorReports('a' + st('\x1b]11;rgb:ffff/ffff/ffff') + 'b'), 'ab');
  assert.equal(stripOscColorReports('\x1b]52;c;abcd\x1b\\'), '\x1b]52;c;abcd\x1b\\');
  assert.equal(stripOscColorReports('hi'), 'hi');
  assert.equal(stripOscColorReports(''), '');
  assert.match(term, /d = stripOscColorReports\(d\);\s*if \(!d\) return;/);
});

test('a draft-retained pending row keeps one start time instead of sorting by the render clock', () => {
  const term = readFileSync(new URL('../legacy-web/term.js', import.meta.url), 'utf8');
  const started = 1_758_000_000;
  const composerDrafts = new Map();
  const context = ctx({T: {pending: []}, S: {sessions: []}, composerDrafts,
    SessionDockCapabilities: {config: {backend: 'rust'}}, SOURCES: {shell: {name: 'SSH'}}});
  for (const name of ['pendingStartedAt', 'rememberComposerSession']) load(context, name, term);
  for (const name of ['pendingUid', 'pendingDraftFirstSeen', 'pendingDraftStartedAt', 'pendingTmuxSessions']) load(context, name);

  // The receipt's `started` wins; a sidebar row only has `created`; nothing at all falls back.
  assert.equal(context.pendingStartedAt({started, created: '2000-01-01T00:00:00.000Z'}), started);
  assert.equal(context.pendingStartedAt({created: new Date(started * 1000).toISOString()}), started);
  assert.equal(context.pendingStartedAt({}, 7), 7);

  // Clicking the row again hands rememberComposerSession a row whose `created`
  // derives from the same start; the first recorded stamp stays.
  const draft = {text: 'rclone config', attachments: [], quotes: [], session: null};
  context.rememberComposerSession(draft, {uid: 'tmux:ssh', name: 'ssh', source: 'shell', cwd: '/w',
    node_id: 'n1', record_id: 'r', instance_id: 'i', started});
  assert.equal(draft.session.started, started);
  context.rememberComposerSession(draft, {uid: 'tmux:ssh', name: 'ssh', source: 'shell', cwd: '/w',
    node_id: 'n1', created: new Date((started + 60) * 1000).toISOString()});
  assert.equal(draft.session.started, started);
  context.rememberComposerSession(draft, {uid: 'tmux:other', name: 'other', source: 'shell', cwd: '/w',
    created: new Date((started + 60) * 1000).toISOString()});
  assert.equal(draft.session.started, started + 60);

  // term/list has dropped the exited SSH instance: the draft-only row is sorted
  // by the saved start, identically on every render.
  draft.session = {uid: 'tmux:ssh', name: 'ssh', source: 'shell', cwd: '/w', node_id: 'n1', started};
  composerDrafts.set('tmux:ssh', draft);
  const first = context.pendingTmuxSessions();
  assert.equal(first.length, 1);
  assert.equal(first[0].updated, new Date(started * 1000).toISOString());
  assert.equal(first[0].state, 'exited');
  assert.deepEqual([first[0].created, first[0].updated], (() => { const r = context.pendingTmuxSessions()[0]; return [r.created, r.updated]; })());

  // A draft saved before `started` existed pins the moment it was first listed.
  composerDrafts.set('tmux:old', {text: 'x', attachments: [], quotes: [],
    session: {uid: 'tmux:old', name: 'old', source: 'shell', cwd: '/w'}});
  const seen = context.pendingTmuxSessions().find(r => r.name === 'old');
  assert.ok(Math.abs(Date.parse(seen.updated) - Date.now()) < 5000);
  assert.equal(context.pendingTmuxSessions().find(r => r.name === 'old').updated, seen.updated);

  // A term/list row without `started` (an older node) is pinned the same way,
  // never re-stamped with the render clock.
  context.T.pending.push({name: 'nostart', source: 'shell', cwd: '/w', record_id: 'r2', instance_id: 'i2',
    running: false, state: 'exited', started: null});
  const row = context.pendingTmuxSessions().find(r => r.name === 'nostart');
  assert.ok(Math.abs(Date.parse(row.updated) - Date.now()) < 5000);
  assert.equal(context.pendingTmuxSessions().find(r => r.name === 'nostart').updated, row.updated);
  assert.equal(context.pendingTmuxSessions().find(r => r.name === 'nostart').created, row.created);
});

test('a turn keeps its first native final as the conclusion when a Stop hook or task report appends work after it', () => {
  const context = ctx({S: {compactTurns: true}});
  for (const name of ['TOOL_ROLES', 'TURN_START_ROLES', 'GROUP_MIN', 'isGroupableTool', 'pairTools',
    'planMessages', 'baseMessageRole', 'isTurnStart', 'sameNativeTurn', 'isTurnAssistant',
    'isFinalAssistant', 'isPassiveTurnTail', 'turnConclusion', 'visiblePlanSize', 'turnKey',
    'planTurnSegment', 'planTurn', 'planTurns']) load(context, name);
  const t = (role, text, extra = {}) => ({role, text, turn_id: 't1', ...extra});
  const tool = (id, text) => [t('tool', `$ ${text}`, {call_id: id}), t('tool_result', text, {call_id: id})];
  const shape = plan => plan.map(item => item.turn
    ? `turn[${item.turn.items.length}${item.turn.hasConclusion ? ',conclusion' : ''}]`
    : item.g ? `group[${item.g.length}]` : `${item.m.role}:${item.m.text}`);
  const work = [t('assistant', '我先读文档', {phase: 'progress'}), ...tool('c1', 'cat a'), ...tool('c2', 'cat b')];
  const review = t('assistant', '复核完成。核心发现…', {phase: 'final'});
  const follow = t('assistant', 'SessionDock 是常驻服务，已标记 WATCHDOG_EXEMPT', {phase: 'final'});
  const next = {role: 'user', text: '好的，写一个安排文档', turn_id: 't2'};

  // 报告场景：长结论 → Stop hook 拒绝收尾 → 一次 echo → 短补充。结论必须留在顶层。
  same(shape(context.planTurns([t('user', '重排优先级'), ...work, review, ...tool('c3', 'echo WATCHDOG_EXEMPT'), follow, next])),
    ['user:重排优先级', 'turn[5,conclusion]', 'assistant:复核完成。核心发现…',
     'tool:$ echo WATCHDOG_EXEMPT', 'assistant:SessionDock 是常驻服务，已标记 WATCHDOG_EXEMPT', 'user:好的，写一个安排文档']);
  // hook 之后的追加工作够长时自成第二个过程合集，并露出自己的收尾。
  same(shape(context.planTurns([t('user', '重排优先级'), ...work, review,
    t('assistant', '补挂看门狗', {phase: 'progress'}), ...tool('c3', 'echo a'), ...tool('c4', 'echo b'), follow, next])),
    ['user:重排优先级', 'turn[5,conclusion]', 'assistant:复核完成。核心发现…', 'turn[5,conclusion]',
     'assistant:SessionDock 是常驻服务，已标记 WATCHDOG_EXEMPT', 'user:好的，写一个安排文档']);
  // 追加工作仍在进行的活动尾段照旧完整铺开，不猜结论。
  same(shape(context.planTurns([t('user', '重排优先级'), ...work, review, ...tool('c3', 'echo a'), ...tool('c4', 'echo b')],
    {tailComplete: false, openTail: true})),
    ['user:重排优先级', 'turn[5,conclusion]', 'assistant:复核完成。核心发现…', 'group[2]']);
  // 后台 task 短报仍按老规则：主 final 是结论，task 事件与短报平铺在后。
  same(shape(context.planTurns([t('user', '重排优先级'), ...work, review,
    t('event', 'task done', {event_kind: 'task'}), t('assistant', '后台任务完成', {phase: 'final'}), next])),
    ['user:重排优先级', 'turn[5,conclusion]', 'assistant:复核完成。核心发现…', 'event:task done', 'assistant:后台任务完成', 'user:好的，写一个安排文档']);
  // 过程太短的轮次整体平铺，追加段仍单独规划。
  same(shape(context.planTurns([t('user', '问'), t('assistant', '答', {phase: 'final'}), ...tool('c3', 'echo a'), follow, next])),
    ['user:问', 'assistant:答', 'tool:$ echo a', 'assistant:SessionDock 是常驻服务，已标记 WATCHDOG_EXEMPT', 'user:好的，写一个安排文档']);
  // 只有一条 final 的普通轮次不受影响。
  same(shape(context.planTurns([t('user', '重排优先级'), ...work, review, next])),
    ['user:重排优先级', 'turn[5,conclusion]', 'assistant:复核完成。核心发现…', 'user:好的，写一个安排文档']);
});
