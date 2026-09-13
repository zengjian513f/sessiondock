import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {test} from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../legacy-web/app.js', import.meta.url), 'utf8');
function load(context, name) {
  const match = new RegExp(`^(?:async )?function ${name}\\(`, 'm').exec(source);
  assert.ok(match, name);
  const end = source.indexOf('\n}\n', match.index);
  vm.runInContext(source.slice(match.index, end + 2), context);
  return context[name];
}
const A = 'a'.repeat(32), B = 'b'.repeat(32), C = 'c'.repeat(32);
const RENDER = ['mediaContinuationEnabled', 'lazyMediaEnabled', 'safeMediaSrc', 'imageHtml', 'mediaMoreInfo', 'mediaMoreHtml', 'mediaGallery'];
const LOAD = ['currentMediaPage', 'validateMediaPage', 'fetchMediaPage', 'mediaPageFailure', 'loadMediaContinuation'];
// Objects created inside the vm realm carry other prototypes; compare structure only.
const same = (actual, expected) => assert.equal(JSON.stringify(actual), JSON.stringify(expected));
const descriptor = index => ({src: '/api/media/' + index.toString(16).padStart(32, '0'), alt: `image ${index}`, lazy: true});
const descriptors = (start, end) => Array.from({length: end - start}, (_, offset) => descriptor(start + offset));

// The small fake DOM only supports what the continuation handlers touch:
// a gallery whose children are either fake elements or inserted HTML strings.
function element(tag, className = '', textContent = '') {
  return {tag, className, textContent, disabled: false, isConnected: true, children: [], dataset: {},
    attributes: {}, parentElement: null,
    setAttribute(name, value) { this.attributes[name] = value; },
    appendChild(item) { this.children.push(item); item.parentElement = this; return item; },
    before(item) {
      const at = this.parentElement.children.indexOf(this);
      this.parentElement.children.splice(at, 0, item);
      item.parentElement = this.parentElement;
    },
    insertAdjacentHTML(position, html) {
      assert.equal(position, 'beforebegin');
      const at = this.parentElement.children.indexOf(this);
      this.parentElement.children.splice(at, 0, {tag: 'html', html});
    },
    querySelector(selector) {
      const cursor = /^\.media-more\[data-media-cursor="([0-9a-f]{32})"\]$/.exec(selector)?.[1];
      const matches = item => cursor ? item.className === 'media-more' && item.dataset?.mediaCursor === cursor
        : '.' + item.className === selector;
      for (const item of this.children) {
        if (matches(item)) return item;
        const nested = item.querySelector?.(selector);
        if (nested) return nested;
      }
      return null;
    },
    contains(item) { return this.children.some(child => child === item || child.contains?.(item)); },
    remove() {
      const parent = this.parentElement;
      if (parent) parent.children.splice(parent.children.indexOf(this), 1);
      this.isConnected = false;
    }};
}

function renderContext(config = {backend: 'rust', media_continuation: true, media_lazy: true}) {
  const context = vm.createContext({AgentHubCapabilities: {config, allows: () => true}, HUB_MODE: false,
    URL, URLSearchParams, TextDecoder, Uint8Array, appUrl: x => x, esc: x => String(x ?? '').replace(/[&<>"]/g, c => ({'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;'}[c]))});
  for (const name of RENDER) load(context, name);
  return context;
}

function fixture(config = {backend: 'rust', media_continuation: true, media_lazy: true}) {
  const message = {role: 'user', text: 'MANY IMAGES', media: descriptors(0, 16), media_more: {remaining: 24, total: 40, cursor: A}};
  const tail = {role: 'assistant', text: 'tail'};
  const entry = {msgs: [message, tail], meta: {uid: 'codex:fixture'}, bytes: 100, total: 2, end: 100, anchor: 'old',
    version: {head: 'old-head'}, activity: null, partial: null};
  const cache = new Map([['codex:fixture', entry]]);
  const box = element('div', 'msgs'), bubble = element('div', 'msg'), gallery = element('div', 'media-gallery');
  const button = element('button', 'media-more', '还有 24 张图片，加载下一批');
  button.dataset.mediaCursor = A;
  box.appendChild(bubble); bubble.appendChild(gallery);
  for (let index = 0; index < 16; index++) gallery.appendChild(element('span', 'media-load'));
  gallery.appendChild(button);
  const context = renderContext(config);
  Object.assign(context, {
    S: {sel: 'codex:fixture', agent: null, cursors: new Map([['codex:fixture', {end: 100, head: 'old-head', anchor: 'old'}]])},
    cache, inflight: {}, viewKey: (uid, agent) => agent ? `${uid}::${agent}` : uid,
    mediaPageRequests: new Map(), AbortController, SYNC_STALL_MS: 20000,
    setTimeout: () => 1, clearTimeout: () => {}, el: element, $: () => box});
  for (const name of LOAD) load(context, name);
  return {context, entry, message, tail, cache, box, gallery, button};
}
function page(start = 16, end = 32, total = 40, cursor = A, next = B) {
  return {data: {media: descriptors(start, end), page: {cursor, next, start, end, total, remaining: total - end}}, bytes: 20};
}
const rendered = gallery => gallery.children.map(item => item.tag === 'html' ? item.html : item.className);

test('only exact Rust media_continuation capability renders the button; other pages keep byte-identical galleries', () => {
  const items = descriptors(0, 2), more = {remaining: 24, total: 40, cursor: A};
  for (const config of [{}, {backend: 'python'}, {backend: 'python', media_continuation: true}, {backend: 'rust'},
    {backend: 'rust', media_continuation: 1}, {backend: 'rust', media_continuation: false}, {backend: 'rust', media_continuation: true}]) {
    const enabled = config.backend === 'rust' && config.media_continuation === true;
    const c = renderContext({...config, media_lazy: true});
    assert.equal(c.mediaContinuationEnabled(), enabled);
    const plain = c.mediaGallery(items), withMore = c.mediaGallery(items, more);
    assert.equal(plain, c.mediaGallery(items, undefined));
    assert.equal(plain, c.mediaGallery(items, null));
    assert.doesNotMatch(plain, /media-more/);
    assert.equal(withMore.includes(`<button type="button" class="media-more" data-media-cursor="${A}"`), enabled);
    assert.equal(withMore.includes('还有 24 张图片，加载下一批</button></div>'), enabled);
    if (!enabled) assert.equal(withMore, plain);
    else assert.equal(withMore.indexOf('<button'), plain.length - '</div>'.length);
    const unavailable = c.mediaGallery(items, {remaining: 3, total: 19, cursor: null});
    assert.equal(unavailable.includes('<button type="button" class="media-more" disabled'), enabled);
    assert.equal(unavailable.includes('还有 3 张图片暂不可加载'), enabled);
    assert.doesNotMatch(unavailable, /data-media-cursor/);
    assert.equal(c.mediaGallery([], enabled ? more : null).includes('media-more'), enabled);
  }
  const c = renderContext();
  for (const invalid of [{remaining: 0, total: 16, cursor: A}, {remaining: -1, total: 40, cursor: A}, {remaining: 1.5, total: 40, cursor: A},
    {remaining: 24, total: 40}, {remaining: 24, total: 40, cursor: 'A'.repeat(32)}, {remaining: 24, total: 40, cursor: A.slice(1)},
    {remaining: 24, total: 40, cursor: 7}, {remaining: 24, total: 'many', cursor: A}, {remaining: 24, total: 23, cursor: A},
    {remaining: 24, total: 2 ** 53, cursor: A}, [24, 40, A], 'more', 1, true]) {
    assert.equal(c.mediaGallery(items, invalid), c.mediaGallery(items), JSON.stringify(invalid));
  }
});

test('page validation rejects wrong cursor, non-progress, inconsistent bounds, next==cursor, oversize and non-object items', () => {
  const {context} = fixture(), validate = context.validateMediaPage, more = {remaining: 24, total: 40, cursor: A};
  assert.equal(validate(page().data, more, A).end, 32);
  assert.equal(validate(page(32, 40, 40, B, null).data, {remaining: 8, total: 40, cursor: B}, B).next, null);
  assert.equal(validate(page(16, 17, 17, A, null).data, {remaining: 1, total: 17, cursor: A}, A).remaining, 0);
  for (const change of [{cursor: B}, {start: 15}, {start: 17}, {end: 33}, {end: 31}, {total: 41}, {total: 39}, {remaining: 9},
    {remaining: 7}, {next: A}, {next: null}, {next: C.toUpperCase()}, {next: C.slice(1)}, {start: 1.5}, {end: Infinity},
    {total: '40'}, {remaining: -0.5}]) {
    const data = page().data; Object.assign(data.page, change);
    assert.throws(() => validate(data, more, A), /不匹配/, JSON.stringify(change));
  }
  for (const change of [{start: 16, end: 16, remaining: 24}, {start: 16, end: 15, remaining: 25}]) {
    const data = page().data; data.media = []; Object.assign(data.page, change);
    assert.throws(() => validate(data, more, A), /不匹配/, JSON.stringify(change));
  }
  const last = page(32, 40, 40, B, null).data; last.page.next = C;
  assert.throws(() => validate(last, {remaining: 8, total: 40, cursor: B}, B));
  const wrongMessage = page().data;
  assert.throws(() => validate(wrongMessage, {remaining: 8, total: 40, cursor: A}, A));
  assert.throws(() => validate(page().data, {remaining: 24, total: 41, cursor: A}, A));
  assert.throws(() => validate(page().data, null, A));
  assert.throws(() => validate(page().data, more, B));
  assert.throws(() => validate(page().data, more, A.toUpperCase()));
  const large = page(16, 33).data; large.page.remaining = 7;
  assert.throws(() => validate(large, more, A), /不匹配/);
  const none = page().data; none.media = [];
  assert.throws(() => validate(none, more, A));
  for (const invalid of [null, 'text', 1, [], [descriptor(16)]]) {
    const data = page().data; data.media[0] = invalid;
    assert.throws(() => validate(data, more, A), /不匹配/, JSON.stringify(invalid));
  }
  for (const shape of [null, [], {media: 'x', page: page().data.page}, {media: descriptors(16, 32)}, {page: page().data.page}]) {
    assert.throws(() => validate(shape, more, A));
  }
});

test('a continuation appends only to its message, advances then removes the button and leaves live cursor fields alone', async () => {
  const {context, entry, message, tail, gallery, button} = fixture();
  const msgs = entry.msgs, media = message.media, calls = [];
  let resolve;
  context.fetchMediaPage = (...args) => { calls.push(args); return new Promise(done => { resolve = done; }); };
  const task = context.loadMediaContinuation('codex:fixture', null, A, button);
  assert.equal(button.disabled, true);
  assert.match(button.textContent, /正在读取/);
  assert.equal(context.mediaPageRequests.size, 1);
  // An SSE append advances the same entry while HTTP is pending.
  const live = {role: 'user', text: 'live'};
  entry.msgs = entry.msgs.concat(live); entry.end = 999; entry.anchor = 'live-anchor'; entry.version = {head: 'live-head'};
  context.S.cursors.set('codex:fixture', {end: 999, head: 'live-head', anchor: 'live-anchor'});
  resolve(page()); await task;
  assert.deepEqual(calls, [['codex:fixture', null, A, calls[0][3]]]);
  assert.ok(calls[0][3] instanceof AbortSignal);
  assert.equal(message.media.length, 32);
  same(message.media.slice(0, 16), media);
  same(message.media.slice(16), descriptors(16, 32));
  same(message.media_more, {remaining: 8, total: 40, cursor: B});
  assert.deepEqual(entry.msgs, [message, tail, live]);
  assert.equal(entry.end, 999); assert.equal(entry.anchor, 'live-anchor'); assert.deepEqual(entry.version, {head: 'live-head'});
  assert.deepEqual(context.S.cursors.get('codex:fixture'), {end: 999, head: 'live-head', anchor: 'live-anchor'});
  assert.equal(msgs.length, 2, 'the original messages array must not be mutated');
  assert.deepEqual(rendered(gallery), [...Array(16).fill('media-load'), descriptors(16, 32).map(item => context.imageHtml(item)).join(''), 'media-more']);
  assert.equal(button.isConnected, true); assert.equal(button.disabled, false);
  assert.equal(button.dataset.mediaCursor, B);
  assert.equal(button.textContent, '还有 8 张图片，加载下一批');
  assert.equal(context.mediaPageRequests.size, 0);

  context.fetchMediaPage = async () => page(32, 40, 40, B, null);
  await context.loadMediaContinuation('codex:fixture', null, B, button);
  assert.equal(message.media.length, 40);
  same(message.media.slice(32), descriptors(32, 40));
  assert.equal('media_more' in message, false);
  assert.equal(button.isConnected, false);
  assert.deepEqual(rendered(gallery).slice(-1), [descriptors(32, 40).map(item => context.imageHtml(item)).join('')]);
  assert.equal(gallery.children.length, 18);
  assert.equal(entry.end, 999); assert.deepEqual(entry.msgs, [message, tail, live]);
  assert.equal(context.mediaPageRequests.size, 0);
});

test('failures keep the message, DOM and cursor, announce the reason and offer a retry that then succeeds', async () => {
  for (const failure of [Object.assign(new Error('specific 409'), {status: 409}), Object.assign(new Error('busy 503'), {status: 503}),
    Object.assign(new Error('AbortError'), {name: 'AbortError'}), 'invalid']) {
    const {context, entry, message, gallery, button} = fixture();
    const before = rendered(gallery);
    context.fetchMediaPage = async () => {
      if (failure === 'invalid') { const reply = page(); reply.data.media[3] = null; return reply; }
      throw failure;
    };
    await context.loadMediaContinuation('codex:fixture', null, A, button);
    assert.equal(message.media.length, 16);
    same(message.media_more, {remaining: 24, total: 40, cursor: A});
    assert.equal(entry.end, 100); assert.equal(entry.anchor, 'old');
    const notice = gallery.querySelector('.media-page-error');
    assert.ok(notice, 'error element');
    assert.equal(notice.attributes.role, 'alert');
    assert.match(notice.textContent, /已加载的图片保持不变/);
    if (failure === 'invalid') assert.match(notice.textContent, /不匹配/);
    else assert.match(notice.textContent, new RegExp(failure.message));
    if (failure.status) assert.match(notice.textContent, new RegExp(`HTTP ${failure.status}`));
    assert.deepEqual(rendered(gallery), [...before.slice(0, -1), 'media-page-error', 'media-more']);
    assert.equal(button.disabled, false);
    assert.equal(button.textContent, '重试加载图片');
    assert.equal(button.dataset.mediaCursor, A);
    assert.equal(context.mediaPageRequests.size, 0);

    context.fetchMediaPage = async () => page();
    await context.loadMediaContinuation('codex:fixture', null, A, button);
    assert.equal(message.media.length, 32);
    assert.equal(gallery.querySelector('.media-page-error'), null);
    assert.equal(button.textContent, '还有 8 张图片，加载下一批');
    assert.equal(button.dataset.mediaCursor, B);
  }
});

test('view change, request reset, cache replacement or cursor change mid-flight discards the page without touching DOM or cache', async () => {
  for (const mutate of [state => { state.context.S.sel = 'other'; }, state => { state.context.S.agent = 'agent'; },
    state => { state.context.inflight = {}; }, state => { state.cache.set('codex:fixture', {...state.entry}); },
    state => { state.message.media_more = {...state.message.media_more, cursor: B}; },
    state => { delete state.message.media_more; }]) {
    const state = fixture(); let resolve;
    const before = rendered(state.gallery);
    state.context.fetchMediaPage = () => new Promise(done => { resolve = done; });
    const task = state.context.loadMediaContinuation('codex:fixture', null, A, state.button);
    mutate(state);
    const expected = JSON.stringify(state.message.media_more);
    resolve(page()); await task;
    assert.equal(state.message.media.length, 16);
    assert.equal(JSON.stringify(state.message.media_more), expected);
    assert.deepEqual(rendered(state.gallery), before);
    assert.equal(state.gallery.querySelector('.media-page-error'), null);
    assert.equal(state.context.mediaPageRequests.size, 0);
  }
  // A failure observed after the view moved on is equally silent.
  const state = fixture(); let reject;
  state.context.fetchMediaPage = () => new Promise((_, fail) => { reject = fail; });
  const task = state.context.loadMediaContinuation('codex:fixture', null, A, state.button);
  state.context.S.sel = 'other'; reject(Object.assign(new Error('late 409'), {status: 409})); await task;
  assert.equal(state.gallery.querySelector('.media-page-error'), null);
  assert.equal(state.message.media.length, 16);
});

test('duplicate clicks, unknown cursors, disabled capability and re-rendered galleries are handled without extra requests', async () => {
  const {context, message, box, gallery, button} = fixture();
  let calls = 0, resolve;
  context.fetchMediaPage = () => { calls++; return new Promise(done => { resolve = done; }); };
  const task = context.loadMediaContinuation('codex:fixture', null, A, button);
  await context.loadMediaContinuation('codex:fixture', null, A, button);
  await context.loadMediaContinuation('codex:fixture', null, B, button);
  await context.loadMediaContinuation('codex:fixture', null, 'A'.repeat(32), button);
  await context.loadMediaContinuation('codex:missing', null, A, button);
  assert.equal(calls, 1);
  // The whole conversation was re-rendered while HTTP was pending: the cache
  // message still receives the page and the replacement button is updated.
  const replacement = element('button', 'media-more', '还有 24 张图片，加载下一批');
  replacement.dataset.mediaCursor = A;
  button.remove(); gallery.appendChild(replacement);
  resolve(page()); await task;
  assert.equal(message.media.length, 32);
  assert.equal(replacement.dataset.mediaCursor, B);
  assert.equal(replacement.textContent, '还有 8 张图片，加载下一批');
  assert.equal(rendered(gallery).at(-2), descriptors(16, 32).map(item => context.imageHtml(item)).join(''));
  // Nothing rendered any more: cache is still updated, DOM untouched, no throw.
  box.children.length = 0;
  context.fetchMediaPage = async () => page(32, 40, 40, B, null);
  await context.loadMediaContinuation('codex:fixture', null, B, replacement);
  assert.equal(message.media.length, 40);
  assert.equal('media_more' in message, false);
  assert.equal(replacement.textContent, '正在读取图片…');

  const off = fixture({backend: 'rust', media_continuation: false, media_lazy: true});
  off.context.fetchMediaPage = async () => { throw new Error('must not fetch'); };
  await off.context.loadMediaContinuation('codex:fixture', null, A, off.button);
  assert.equal(off.message.media.length, 16); assert.equal(off.button.disabled, false);
  assert.equal(off.context.mediaGallery(off.message.media, off.message.media_more).includes('media-more'), false);
});

test('page fetch uses no-store, surfaces server errors with status and bounds the body at 1 MiB', async () => {
  const {context} = fixture();
  const seen = [];
  context.fetch = async (url, options) => {
    seen.push([url, options]);
    return {ok: false, status: 409, json: async () => ({error: 'timeline changed'})};
  };
  await assert.rejects(context.fetchMediaPage('codex:fixture', 'agent one', A, 'signal'),
    error => error.message === 'timeline changed' && error.status === 409);
  assert.equal(seen[0][0], `api/messages/codex%3Afixture/media-page?cursor=${A}&agent=agent+one`);
  same(seen[0][1], {signal: 'signal', cache: 'no-store'});
  context.fetch = async () => ({ok: false, status: 503, json: async () => { throw new Error('not json'); }});
  await assert.rejects(context.fetchMediaPage('codex:fixture', null, A), error => error.message === 'HTTP 503' && error.status === 503);
  let cancelled = 0;
  context.fetch = async () => ({ok: true, status: 200, body: {getReader: () => ({
    read: async () => ({done: false, value: new Uint8Array(512 * 1024 + 1)}), cancel: async () => { cancelled++; }})}});
  await assert.rejects(context.fetchMediaPage('codex:fixture', null, A), /读取预算/);
  assert.equal(cancelled, 1);
  const encoded = new TextEncoder().encode(JSON.stringify(page().data));
  let served = false;
  context.fetch = async () => ({ok: true, status: 200, body: {getReader: () => ({
    read: async () => served ? {done: true} : (served = true, {done: false, value: encoded}), cancel: async () => {}})}});
  const {data, bytes} = await context.fetchMediaPage('codex:fixture', null, A);
  assert.equal(bytes, encoded.length); assert.equal(data.page.cursor, A); assert.equal(data.media.length, 16);
});
