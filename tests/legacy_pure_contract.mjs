import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {test} from 'node:test';
import vm from 'node:vm';

const app = readFileSync(new URL('../legacy-web/app.js', import.meta.url), 'utf8');
const nodes = readFileSync(new URL('../legacy-web/nodes.js', import.meta.url), 'utf8');
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
  return vm.createContext({URL, URLSearchParams, AgentHubCapabilities: {config: {}, allows: () => false},
    HUB_MODE: false, APP_BASE: new URL('http://127.0.0.1:8080/agenthub/'), DEBUG_RUN: '',
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
  const base = {APP_BASE: new URL('http://127.0.0.1:8080/agenthub/')};
  assert.equal(url(base, '/api/media/' + A), `http://127.0.0.1:8080/agenthub/api/media/${A}`);
  assert.equal(url(base, 'api/sessions'), 'http://127.0.0.1:8080/agenthub/api/sessions');
  assert.equal(url(base, ''), 'http://127.0.0.1:8080/agenthub/');
  assert.equal(url(base, 'https://evil.example/x'), 'https://evil.example/x');
  assert.equal(url({...base, DEBUG_RUN: 'run_1'}, 'api/sessions'), 'http://127.0.0.1:8080/agenthub/api/sessions?debug_run=run_1');
  assert.equal(url({...base, DEBUG_RUN: 'run_1'}, 'index.html').includes('debug_run'), false);
  assert.equal(url({...base, HUB_MODE: true}, 'api/search'), 'http://127.0.0.1:8080/agenthub/api/search?nodes=n1%2Cn2');
  assert.equal(url({...base, HUB_MODE: true}, 'api/trash/purge'), 'http://127.0.0.1:8080/agenthub/api/trash/purge?nodes=n1%2Cn2');
  assert.equal(url({...base, HUB_MODE: true}, 'api/search?nodes=keep'), 'http://127.0.0.1:8080/agenthub/api/search?nodes=keep');
  assert.equal(url({...base, HUB_MODE: true}, 'api/sessions').includes('nodes='), false);
  assert.equal(url({...base, HUB_MODE: true}, 'api/term/complete-dir'), 'http://127.0.0.1:8080/agenthub/api/term/complete-dir?node=nid');
  assert.equal(url({...base, HUB_MODE: false}, 'api/search').includes('nodes='), false);
  assert.equal(url(base, null), 'http://127.0.0.1:8080/agenthub/null');
});

test('only exact Rust boolean flags enable history pages, lazy media and continuation', () => {
  const flags = [['historyPagesEnabled', 'history_pages'], ['lazyMediaEnabled', 'media_lazy'],
    ['mediaContinuationEnabled', 'media_continuation']];
  for (const config of [{}, {backend: 'python'}, {backend: 'python', history_pages: true, media_lazy: true, media_continuation: true},
    {backend: 'rust'}, {backend: 'rust', history_pages: 1, media_lazy: 1, media_continuation: 1},
    {backend: 'rust', history_pages: false, media_lazy: false, media_continuation: false},
    {backend: 'rust', history_pages: 'true', media_lazy: 'true', media_continuation: 'true'},
    {backend: 'rust', history_pages: true, media_lazy: true, media_continuation: true}]) {
    const c = ctx({AgentHubCapabilities: {config}});
    for (const [name, flag] of flags) {
      load(c, name);
      assert.equal(c[name](), config.backend === 'rust' && config[flag] === true, `${name} ${JSON.stringify(config)}`);
    }
  }
});

test('safeMediaSrc admits local tokens, hub paths only when not lazy, remote http(s) only when allowed', () => {
  const src = (config, extra = {}) => fn('safeMediaSrc', {
    AgentHubCapabilities: {config, allows: name => extra.allows === true || extra.allows?.[name] === true},
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
      AgentHubCapabilities: {config: {backend: 'rust'}, allows: () => true},
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
  assert.equal(reason({T: {listLoaded: true, enabled: true, ended: new Map([['u', {reason: '已退出'}]])}}, 'u'), '已退出');
  assert.equal(reason({AgentHubCapabilities: {config: {backend: 'python'}, allows: () => true},
    T: {listLoaded: true, enabled: true, ended: new Map([['u', {reason: '已退出'}]]), sources: {codex: true}},
    linkedTermSession: () => ({name: 't'})}, 'u'), '');
  assert.match(reason({T: {listLoaded: true, enabled: true, pending: [{record_id: 'r', name: 'p', stale: true}],
    ended: new Map()}, pendingUid: () => 'u'}, 'u'), /创建实例尚未就绪/);
  assert.equal(reason({T: {listLoaded: true, enabled: true, pending: [{record_id: 'r', name: 'p', stale: true,
    unavailable_reason: '还在准备'}], ended: new Map()}, pendingUid: () => 'u'}, 'u'), '还在准备');
  assert.match(reason({T: {listLoaded: true, enabled: false, ended: new Map(), pending: []}}, 'u'), /未返回具体原因/);
  assert.match(reason({linkedTermSession: () => null, AgentHubCapabilities: {config: {backend: 'rust'}, allows: () => false},
    T: {listLoaded: true, enabled: true, ended: new Map(), pending: [], resume_sources: {}}}, 'codex:u'), /不能按名称猜测关联/);
  assert.match(reason({AgentHubCapabilities: {config: {backend: 'python'}, allows: () => true},
    linkedTermSession: () => null, T: {listLoaded: true, enabled: true, sources: {}, ended: new Map(), pending: []}}, 'codex:u'),
    /未找到可用的 Codex 命令/);
  assert.equal(reason({ConsoleUI: {errors: new Map([['u', '上次失败']]), busy: new Set()}}, 'u', null, true), '上次失败');
  assert.equal(reason({ConsoleUI: {errors: new Map([['u', '上次失败']]), busy: new Set()}}, 'u', null, false), '');
});

test('nest tree: spawned_by nests by node/source/sid, cycles stay roots, missing fields degrade to flat', () => {
  const S = {nest: true, nestClosed: new Set(), live: new Set()};
  const context = ctx({S});
  for (const name of ['spawnKey', 'spawnParentOf', 'nestTree', 'nestStamp', 'agentRunning', 'expandRows']) load(context, name);
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
  for (const name of ['spawnKey', 'spawnParentOf', 'nestTree', 'sessionContinued', 'sessionHidden']) load(context, name);
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
  same(badge('pending', 'agenthub-x'), {classes: ['tmux', 'visible'], text: '', title: 'tmux 会话运行中'});
});

test('liveStatusTitle says unknown without the live capability and tmux/direct with it', () => {
  const rust = fn('liveStatusTitle', {AgentHubCapabilities: {config: {backend: 'rust'}, allows: () => false}});
  assert.equal(rust(false), '运行状态未知，尚未实现进程探测');
  assert.equal(rust(true), '运行状态未知，尚未实现进程探测');
  const python = fn('liveStatusTitle', {AgentHubCapabilities: {config: {}, allows: () => true}});
  assert.equal(python(false), '运行中');
  assert.equal(python(true), '运行于 tmux');
});
