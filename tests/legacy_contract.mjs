import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {test} from 'node:test';
import vm from 'node:vm';

const read = name => readFileSync(new URL(`../legacy-web/${name}`, import.meta.url), 'utf8');
const capabilitiesSource = read('capabilities.js');
const appSource = read('app.js');
const disabled = {backend: 'rust', read_only: true, live: false,
  outbox: false, audit: false, search: false, files: false};

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
  const context = contextWithCapabilities(disabled, {T: {}, TERM_PAGE_ID: 'page',
    confirm: message => { prompts.push(message); return true; },
    post: async (_path, body) => {calls.push(body); return calls.length === 1 ? {conflict: true, owner: {ip: '10.66.66.1', label: ''}} : {token: 'lease'};}});
  loadFunction(context, 'describeTermTaker', read('term.js'));
  const claim = loadFunction(context, 'claimTermOwnership', read('term.js'));
  assert.equal(await claim('name', 'codex:uid', {uid: 'codex:uid', instance_id: 'captured-instance'}), 'lease');
  for (const body of calls) {
    assert.equal(body.uid, 'codex:uid');
    assert.equal(body.instance_id, 'captured-instance');
  }
  assert.equal(calls[1].force, true);
  // Without a label, and without the server marking the holder elsewhere, the
  // prompt names no address (an older node's hub tunnel address says nothing).
  assert.equal(prompts[0], '该终端正由另一页面控制。\n\n是否抢占终端？');
});

test('an automatic pty restore never asks to take over a held terminal', async () => {
  const calls = [];
  const context = contextWithCapabilities(disabled, {T: {}, TERM_PAGE_ID: 'page',
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

test('terminal takeover prompts describe the taker only by what is meaningful', () => {
  const context = contextWithCapabilities(disabled, {});
  const describe = loadFunction(context, 'describeTermTaker', read('term.js'));
  assert.equal(describe('', ''), '另一页面');
  assert.equal(describe('', '203.0.113.7'), '另一页面（203.0.113.7）');
  assert.equal(describe('iPhone · Safari', ''), ' iPhone · Safari ');
  assert.equal(describe('iPhone · Safari', '203.0.113.7'), ' iPhone · Safari（203.0.113.7） ');
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

test('cross-kind open is rejected before changing persisted layout or rendered pane', async () => {
  const errors=new Map();
  const T={uid:'codex:native',views:new Map([['pane',{bindingUid:'tmux:pane'}]])};
  const context=contextWithCapabilities(disabled,{T,ConsoleUI:{errors},renderTakeoverBtn:()=>{},
    $:()=>assert.fail('A rejected open must not mutate the pane'),
    rememberTermOpen:()=>assert.fail('A rejected open must not change layout')});
  loadFunction(context,'termBindingServes',read('term.js'));
  const open=loadFunction(context,'openTermPane',read('term.js'));
  assert.equal(await open('pane'),false);
  assert.match(errors.get('codex:native'),/先.*释放/);
});

test('Rust terminal lookup follows a Codex rollback branch to the pane bound to its ancestor', () => {
  const node = 'n'.repeat(32);
  const parent = {uid: 'codex:a', source: 'codex', sid: 'sid-a', fork_parent: true, created: '2026-09-14T00:00:00Z'};
  const branch = {uid: 'codex:b', source: 'codex', sid: 'sid-b', forked_from_id: 'sid-a', created: '2026-09-15T00:00:00Z'};
  const elsewhere = {uid: `codex:${node}~b`, node_id: node, source: 'codex', sid: 'sid-b', forked_from_id: 'sid-a'};
  const S = {sessions: [parent, branch, elsewhere], live: new Set(['codex:b'])};
  const T = {list: [{name: 'pane', uid: 'codex:a', instance_id: 'instance'}], pending: [], uid: null, name: null};
  const context = contextWithCapabilities(disabled, {T, S});
  for (const name of ['forkAncestors', 'forkLeaf', 'forkLeafUid']) loadFunction(context, name);
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

test('operator binding explanation separates native association from delivery and exit', () => {
  const context=contextWithCapabilities(disabled);
  const message=loadFunction(context,'pendingBindingMessage',read('term.js'));
  assert.match(message({binding:{state:'confirmed',uid:'codex:full'}}),/操作者.*codex:full/);
  assert.match(message({binding:{state:'confirmed',uid:'codex:full'}}),/不是可靠发送确认/);
  assert.match(message({binding:{state:'uncertain',uid:'codex:full'}}),/尚未确认/);
  assert.equal(message({unavailable_reason:'取消仍未确认退出',binding:{state:'confirmed',uid:'codex:full'}}),'取消仍未确认退出');
  // A process-evidence binding is named as such, still not delivery.
  const process = message({binding:{state:'confirmed',method:'process',uid:'codex:full'}});
  assert.match(process, /进程证据.*codex:full/);
  assert.match(process, /不是可靠发送确认/);
  assert.match(message({}), /进程证据自动关联/);
});

test('bug-report worker rows surface the manifest status and pending rows name their state', () => {
  const context=contextWithCapabilities({...disabled, backend:'rust'});
  const source=read('term.js');
  const table=/const WORKER_STATUS_TEXT = \{[\s\S]*?\n\};/.exec(source);
  assert.ok(table, 'WORKER_STATUS_TEXT table exists');
  vm.runInContext(table[0], context);
  const worker=loadFunction(context,'workerStatusMessage',source);
  assert.equal(worker({kind:'bug-report',worker_status:'failed',worker_error:'未注入'}),'提示词注入失败：未注入');
  assert.equal(worker({kind:'bug-report',worker_status:'submitted'}),'提示词已提交');
  assert.equal(worker({worker_status:'failed'}),'');
  context.workerStatusMessage=worker;
  const message=loadFunction(context,'pendingBindingMessage',source);
  assert.match(message({kind:'bug-report',worker_status:'failed',worker_error:'未注入',declared_sid:'abc'}),/提示词注入失败：未注入/);
  const label=loadFunction(context,'pendingStateLabel',source);
  assert.equal(label({record_id:'r',state:'exited'}),'实例已退出');
  assert.equal(label({record_id:'r',state:'running'}),'等待首条消息');
  assert.equal(label({record_id:'r',state:'running',kind:'bug-report',worker_status:'injecting'}),'正在注入缺陷报告提示词');
  assert.match(source, /info\.state === 'uncertain'[\s\S]*?`状态不确定 · \$\{info\.title/);
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
  await loadFunction(context, 'submitComposer', read('term.js'))();
  assert.match(messages[0], /可靠发送尚未启用/);
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

test('pending composer attachments carry the exact Rust launch receipt identity', async () => {
  const row={name:'node~pending-host',record_id:'receipt',instance_id:'instance'};
  let request;
  const context=contextWithCapabilities(disabled, {
    T:{pending:[row]}, pendingUid:name=>`tmux:${name}`,
    URL, appUrl:path=>`http://sessiondock.test/${path}`, renderComposerItems:()=>{},
    fetch:async (url, options) => {
      request={url:String(url),options};
      return {ok:true,status:200,json:async()=>({ok:true,attachment_id:'1'})};
    },
  });
  const identity=loadFunction(context,'composerAttachmentIdentity',read('term.js'));
  context.composerAttachmentIdentity=identity;
  assert.deepEqual(
    JSON.parse(JSON.stringify(identity('tmux:node~pending-host'))),
    {uid:'tmux:node~pending-host',record_id:'receipt',instance_id:'instance'});
  assert.deepEqual(JSON.parse(JSON.stringify(identity('codex:node~native'))),
    {uid:'codex:node~native'});
  const upload=loadFunction(context,'uploadComposerAttachment',read('term.js'));
  await upload({file:{name:'新会话附件.txt',type:'text/plain'},status:'',error:'',uploaded:null},
    'tmux:node~pending-host');
  const query=new URL(request.url).searchParams;
  assert.equal(query.get('uid'),'tmux:node~pending-host');
  assert.equal(query.get('record_id'),'receipt');
  assert.equal(query.get('instance_id'),'instance');
  assert.equal(query.get('name'),'新会话附件.txt');
  assert.equal(request.options.method,'POST');
  const python=contextWithCapabilities(undefined, {T:{pending:[row]},pendingUid:name=>`tmux:${name}`});
  assert.deepEqual(JSON.parse(JSON.stringify(
    loadFunction(python,'composerAttachmentIdentity',read('term.js'))('tmux:node~pending-host'))),
    {uid:'tmux:node~pending-host'});
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

test('files pages key their preferences under the namespace', () => {
  const source = read('files.js');
  assert.match(source, /const storageKey = key => namespace \+ 'files-' \+ key;/);
  for (const key of ['view', 'clipboard']) assert.match(source, new RegExp(`store\\('${key}'`));
  assert.match(source, /const historyKey = 'history:' \+ context\.toString\(\);/);
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
  assert.match(read('file.js') + read('files.js'), /· SessionDock'/);
});

test('all pages load the optional contract before their consumers', () => {
  for (const page of ['index.html', 'file.html', 'files.html']) {
    const html = read(page);
    assert.equal(html.match(/src="capabilities\.js/g).length, 1);
    assert.ok(html.indexOf('src="capabilities.js') < html.indexOf('src="typography.js'));
  }
  assert.match(read('nodes.js'), /const STORAGE_PREFIX = SessionDockCapabilities\.namespace/);
  assert.match(read('index.html'), /id="backend-notice" hidden role="status"/);
  assert.match(appSource, /#session-active'\)\.textContent = known \? active : '\?'/);
});

test('file resolution is gated and Python console availability remains unchanged', () => {
  assert.match(read('file.js'), /if \(!SessionDockCapabilities\.allows\('files'\)\) throw new Error/);
  assert.match(read('files.js'), /if \(!SessionDockCapabilities\.allows\('files'\)\) throw new Error/);
  assert.match(appSource, /if \(!SessionDockCapabilities\.allows\('files'\)\) throw new Error/);
  const baseline = readFileSync(new URL('../reference/legacy-web/nodes.js', import.meta.url), 'utf8');
  const start = 'function consoleUnavailableReason';
  const rustGuard = "  // Rust: an unlinked session can only be resumed through an explicitly\n  // configured resume-capable CLI profile; otherwise no name-based guessing.\n  if (SessionDockCapabilities.config.backend === 'rust' && !linked\n      && !(SessionDockCapabilities.allows('terminal_takeover')\n        && cap?.resume_sources?.[sessionTermMeta(uid)?.source || String(uid).split(':')[0]]))\n    return '该会话没有通过完整 UID 和实例校验的运行中终端；不能按名称猜测关联。';\n";
  assert.ok(read('nodes.js').includes(rustGuard));
  const exitGuard = "  if (SessionDockCapabilities.config.backend === 'rust' && T.ended?.has(uid)) {\n    // An exited instance leaves the button as \"接管会话\"\n    // whenever the source has a resume-capable CLI profile (the click starts\n    // a fresh `--resume`); only an unresumable source keeps the gray\n    // explanation. The exited xterm is never reclaimed automatically.\n    const source = sessionTermMeta(uid)?.source || String(uid).split(':')[0];\n    const resumable = !String(uid).startsWith('tmux:') && cap?.enabled\n      && SessionDockCapabilities.allows('terminal_takeover') && !!cap?.resume_sources?.[source]\n      && !linkedTermSession(uid, {followReplacement: true});\n    if (!resumable) return T.ended.get(uid).reason;\n  }\n";
  assert.ok(read('nodes.js').includes(exitGuard));
  const pendingGuard = "  if (SessionDockCapabilities.config.backend === 'rust') {\n    const pending = T.pending?.find(row => row.record_id && pendingUid(row.name) === uid);\n    if (pending?.stale) return pending.unavailable_reason || '创建实例尚未就绪，不能连接控制台。';\n  }\n";
  assert.ok(read('nodes.js').includes(pendingGuard));
  const compatible = read('nodes.js').replace(rustGuard, '').replace(exitGuard, '').replace(pendingGuard, '');
  assert.equal(compatible.slice(compatible.indexOf(start)), baseline.slice(baseline.indexOf(start)));
});

test('pending session header actions use icons when promoted from the overflow menu', () => {
  const term = read('term.js');
  const index = read('index.html');
  assert.match(term, /id="a-native-bind" title="关联原生会话"\s+aria-label="关联原生会话">\$\{uiIcon\('link'\)\}<\/button>/);
  assert.match(term, /id="a-pending-release" title="释放本页控制台"\s+aria-label="释放本页控制台">\$\{uiIcon\('log-out'\)\}<\/button>/);
  assert.match(index, /<symbol id="i-link"/);
  assert.match(index, /<symbol id="i-log-out"/);
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
