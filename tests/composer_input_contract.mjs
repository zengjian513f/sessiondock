/**
 * Contract for composer CHECK helpers in legacy-web/term.js.
 * grok-4.6 headless draft; reviewed by the lead before integration
 */
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {test} from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../legacy-web/term.js', import.meta.url), 'utf8');
const STATES = ['ready', 'starting', 'blocked', 'unknown'];
const NAMES = ['composerInputStatus', 'composerInputAllowsSend'];

function extractFunction(name) {
  const match = new RegExp(`^(?:async )?function ${name}\\(`, 'm').exec(source);
  if (!match) return null;
  const start = source.indexOf('{', match.index);
  if (start < 0) return null;
  let depth = 0, quote = '';
  for (let i = start; i < source.length; i++) {
    const c = source[i];
    if (quote) {
      if (c === '\\') { i++; continue; }
      if (c === quote) quote = '';
      continue;
    }
    if (c === '"' || c === "'" || c === '`') { quote = c; continue; }
    if (c === '{') depth++;
    else if (c === '}') {
      depth--;
      if (depth === 0) return source.slice(match.index, i + 1);
    }
  }
  return null;
}

function load() {
  const context = vm.createContext({});
  for (const name of NAMES) {
    const code = extractFunction(name);
    assert.ok(code, name);
    vm.runInContext(`${code}; this[${JSON.stringify(name)}] = ${name};`, context);
  }
  return context;
}

function shape(actual) {
  assert.equal(typeof actual, 'object');
  assert.ok(actual);
  assert.ok(STATES.includes(actual.state), String(actual.state));
  assert.equal(typeof actual.code, 'string');
  assert.equal(typeof actual.message, 'string');
}

function expectStatus(actual, expected) {
  shape(actual);
  assert.equal(actual.state, expected.state);
  if (Object.hasOwn(expected, 'code')) assert.equal(actual.code, expected.code);
  if (Object.hasOwn(expected, 'message')) assert.equal(actual.message, expected.message);
}

function expectUnknown(actual) {
  expectStatus(actual, {state: 'unknown'});
  assert.ok(actual.message.length > 0);
}

test('composerInputStatus normalizes CHECK responses and fail-closes to unknown', () => {
  const {composerInputStatus: status, composerInputAllowsSend: allows} = load();
  const ready = {state: 'ready', code: '', message: ''};
  expectStatus(status({ok: true, input: {...ready}}), ready);
  expectStatus(status({ok: true, input: {state: 'ready', code: '', message: '可发送'}, draft_revision: 4}),
    {state: 'ready', code: '', message: '可发送'});
  expectStatus(status({ok: true}), {state: 'ready'});
  expectStatus(status({ok: true, draft_revision: 9}), {state: 'ready'});
  assert.equal(allows(status({ok: true})), true);
  assert.equal(allows(status({ok: true, input: {...ready}})), true);

  const structured = [
    {state: 'starting', code: 'cli_starting', message: 'CLI 正在启动，输入已保留'},
    {state: 'starting', code: 'cli_catching_up', message: '终端画面正在同步，输入已保留'},
    {state: 'starting', code: 'cli_pasting', message: 'CLI 正在处理粘贴，输入已保留'},
    {state: 'blocked', code: 'cli_question', message: '请先回答选择题'},
    {state: 'unknown', code: 'cli_not_ready', message: '未识别到编辑区，输入已保留'},
  ];
  for (const input of structured) {
    expectStatus(status({ok: false, input, code: input.code, error: input.message}), input);
    expectStatus(status({ok: true, input}), input);
    expectStatus(status({input}), input);
    assert.equal(allows(status({input})), false);
  }

  for (const input of ['ready', 1, true, [], {state: 'READY', code: '', message: ''},
    {state: 'idle', code: '', message: ''}, {code: 'cli_starting', message: 'x'},
    {state: 'ready', code: 1, message: ''}, {state: 'blocked', code: 'cli_question', message: 1}, {}]) {
    expectUnknown(status({ok: true, input}));
    assert.equal(allows(status({ok: true, input})), false);
  }
  expectUnknown(status({ok: true, input: null}));
  expectUnknown(status({ok: false, input: {...ready}}));
  expectUnknown(status({ok: true, error: '冲突', input: {...ready}}));
  expectUnknown(status({ok: 1}));
  expectUnknown(status({ok: 'true'}));
  expectUnknown(status({ok: false}));
  expectUnknown(status({}));
  for (const data of [undefined, null]) {
    expectUnknown(status(data));
    assert.equal(allows(status(data)), false);
  }

  expectStatus(status({ok: true, error: '冲突'}), {state: 'unknown', message: '冲突'});
  expectStatus(status({ok: true, error: '启动中', code: 'cli_starting'}),
    {state: 'starting', code: 'cli_starting', message: '启动中'});
  assert.equal(allows(status({ok: true, error: '启动中', code: 'cli_starting'})), false);

  const legacy = [
    [{error: 'CLI 正在启动', code: 'cli_starting'}, 'starting'],
    [{error: '画面未追上', code: 'cli_catching_up'}, 'starting'],
    [{error: '正在粘贴', code: 'cli_pasting'}, 'starting'],
    [{error: '请先回答', code: 'cli_question'}, 'blocked'],
    [{error: '未识别编辑区', code: 'cli_not_ready'}, 'unknown'],
    [{error: '服务暂时不可用', code: 'conversation_send'}, 'unknown'],
    [{error: '请求超时', code: 'timeout'}, 'unknown'],
    [{error: '网络中断'}, 'unknown'],
  ];
  for (const [data, state] of legacy) {
    expectStatus(status(data), {state, ...(data.code ? {code:data.code} : {}), message: data.error});
    assert.equal(allows(status(data)), false);
  }
});

test('composerInputAllowsSend is true only when status.state is ready', () => {
  const {composerInputStatus: status, composerInputAllowsSend: allows} = load();
  assert.equal(allows({state: 'ready'}), true);
  assert.equal(allows({state: 'ready', code: 'cli_question', message: 'ignored'}), true);
  assert.equal(allows(status({ok: true})), true);
  for (const value of STATES.filter(state => state !== 'ready').map(state => ({state}))) {
    assert.equal(allows(value), false, value.state);
    assert.equal(allows(status({input: {state: value.state, code: '', message: 'x'}})), false);
  }
  for (const value of [{state: 'READY'}, {state: 'idle'}, {state: ''}, {}, null, undefined, 'ready',
    0, false, {code: 'ready'}]) {
    assert.equal(allows(value), false);
  }
});

// Lead review addition: reversed HTTP completion must not restore old readiness.
test('older polls cannot overwrite a newer SEND check or replacement terminal', async () => {
  const draft = {}, replies = [], updates = [];
  let terminal = 'original';
  const context = vm.createContext({
    composerDrafts: new Map([['uid', draft]]), composerDraftOwner: uid => uid,
    takenOver: () => terminal, termSendLease: () => ({}),
    post: () => new Promise(resolve => replies.push(resolve)),
    updateComposerInputStatus: (uid, data) => updates.push(data),
  });
  vm.runInContext(extractFunction('probeComposerInput'), context);
  const older = context.probeComposerInput('uid');
  const newer = context.probeComposerInput('uid');
  replies[1]({ok:true}); await newer;
  replies[0]({error:'old login screen', code:'cli_not_ready'}); await older;
  assert.equal(updates.length, 1);
  assert.equal(updates[0].ok, true);
  const replaced = context.probeComposerInput('uid');
  terminal = 'replacement';
  replies[2]({ok:true}); await replaced;
  assert.equal(updates.length, 1);
});
