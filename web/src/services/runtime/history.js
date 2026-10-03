import * as Conversation from '../../migration/conversation'
import {viewKey} from '../../domain/runtime/messages.js'

// Finite pages own neither the live append checkpoint nor the watch connection.
// Requests remain raw; the established validation/render callbacks are explicit.
export function createHistoryController(environment, state, messageCache, reader, historyReader, sessionOpen, sync, scroll, validateHistoryPage, presentation) {
  const {capabilities} = environment;
  const {selection, unread} = state;
  const {cache, trimCache} = messageCache;
  const cachePut = (...args) => messageCache.cachePut(...args);
  const fetchMessages = (...args) => reader.fetchMessages(...args);
  const fetchHistoryPage = (...args) => historyReader.fetchHistoryPage(...args);
  const {requests} = sessionOpen;
  const {applyDiff, closeWatch, watchSession} = sync;
  const {renderSession, progress, progressDone, el} = presentation;
  const $ = selector => document.querySelector(selector);
function historyPagesEnabled() {
  return capabilities.config.backend === 'rust'
    && capabilities.config.history_pages === true;
}

const historyPageRequests = new Map();
// 点一次“加载中间 N 条”后自动连续翻页直到缺口填满（分页取但不停）。
// 按钮显示进度，再点一次中止。用 let 是为了浏览器 E2E
// 能关掉连续翻页，逐页检验竞争。
const timing = {chain: true};

function currentHistoryPage(request) {
  return selection.sel === request.uid && selection.agent === request.agent
    && requests.inflight === request.viewRequest && cache.get(request.key) === request.entry
    && request.entry.partial?.cursor === request.cursor;
}

const HISTORY_PAGE_MAX_EVENTS = 10000;   // 服务端 SESSIONDOCK_HISTORY_PAGE_EVENTS 的上限

function historyPageFailure(request, button, error) {
  if (!currentHistoryPage(request)) return;
  Conversation.gapState({gapError:`未改变当前历史快照。${error.message || '历史分页读取失败。'}`,gapDisabled:false,gapLabel:'重试加载这一页',gapReloadBusy:false});
}

function restoreHistoryPageScroll(top, scrollTop, entry) {
  const box = $('#msgs');
  if (!box) return;
  const restore = () => {
    if (box !== $('#msgs') || cache.get(viewKey(selection.sel, selection.agent)) !== entry) return;
    scroll.state.stick = false;
    scroll.state.selfScroll = true;
    const gap = box.querySelector('.history-gap');
    box.scrollTop = gap ? box.scrollTop + gap.getBoundingClientRect().top - top : scrollTop;
    scroll.state.lastTop = box.scrollTop;
    requestAnimationFrame(() => { scroll.state.selfScroll = false; });
  };
  restore();
  requestAnimationFrame(() => { restore(); requestAnimationFrame(restore); });
}

async function loadHistoryPage(uid, agent, button) {
  agent = agent || null;
  const key = viewKey(uid, agent), entry = cache.get(key);
  if (!historyPagesEnabled() || !entry?.partial) return;
  const previous = historyPageRequests.get(key);
  if (previous && currentHistoryPage(previous)) {
    // 连续翻页进行中再点一次 = 中止；已取到的页保留。
    if (previous.chain && previous.ac) { previous.aborted = true; previous.ac.abort(); }
    return;
  }
  if (previous) { previous.ac?.abort(); historyPageRequests.delete(key); }
  const request = {key, uid, agent, entry, cursor: entry.partial.cursor, viewRequest: requests.inflight,
    chain: timing.chain, aborted: false};
  if (!currentHistoryPage(request)) return;
  const ac = new AbortController();
  request.ac = ac;
  let timer = setTimeout(() => ac.abort(), sync.timing.syncStallMs);
  historyPageRequests.set(key, request);
  const target = Number(entry.partial.omitted || 0);
  let loaded = 0;
  const gap = button.parentElement, box = $('#msgs');
  const top = gap.getBoundingClientRect().top, scrollTop = box?.scrollTop || 0;
  // 连续翻页时按钮保持可点（用于中止），并显示进度。
  Conversation.gapState({gapDisabled:!request.chain,gapError:'',gapLabel:request.chain ? `正在加载历史… 0 / ${target.toLocaleString()} 条 · 点击中止` : '正在读取这一页…'});
  let changed = false, failure = null;
  try {
    for (;;) {
      if (!/^[0-9a-f]{32}$/.test(request.cursor || '')) throw new Error('历史分页凭据缺失，请重新载入当前历史。');
      const {data, bytes} = await fetchHistoryPage(uid, agent, request.cursor, ac.signal, entry.partial);
      if (!currentHistoryPage(request)) return;
      const page = validateHistoryPage(data, entry.partial, request.cursor);
      // Ordinary SSE appends may have advanced this same entry while HTTP was
      // pending. Keep that tail and every live cursor field exactly as observed.
      Conversation.pages.insertHistoryPage(entry,data,page,bytes);
      changed = true;
      loaded += data.messages.length;
      request.cursor = entry.partial?.cursor || null;
      if (!request.chain || !entry.partial) break;
      if (button.isConnected) Conversation.gapState({gapLabel:`正在加载历史… ${loaded.toLocaleString()} / ${target.toLocaleString()} 条 · 点击中止`});
      clearTimeout(timer);
      timer = setTimeout(() => ac.abort(), sync.timing.syncStallMs);
    }
  } catch (error) {
    // 用户中止连续翻页：已取到的页保留，按钮回到剩余数；其余失败照常提示。
    if (!(error?.name === 'AbortError' && request.aborted)) failure = error;
  } finally {
    clearTimeout(timer);
    if (historyPageRequests.get(key) === request) historyPageRequests.delete(key);
  }
  if (changed && currentHistoryPageEntry(request)) {
    trimCache();
    await renderSession(entry.meta, entry.msgs, entry.activity, {startWatch: false, historyPageEntry: entry});
    if (selection.sel === uid && selection.agent === agent && cache.get(key) === entry && requests.inflight === request.viewRequest) {
      restoreHistoryPageScroll(top, scrollTop, entry);
      if (failure) historyPageFailure(request, $('#msgs .history-gap-load'), failure);
    }
  } else if (failure) historyPageFailure(request, button, failure);
}

/** 页已取到、只等渲染：视图与缓存条目仍是发起时的那份即可（游标已推进）。 */
function currentHistoryPageEntry(request) {
  return selection.sel === request.uid && selection.agent === request.agent
    && requests.inflight === request.viewRequest && cache.get(request.key) === request.entry;
}

async function reloadHistoryWindow(uid, agent, button) {
  agent = agent || null;
  const key = viewKey(uid, agent), entry = cache.get(key), viewRequest = requests.inflight;
  if (!historyPagesEnabled() || !entry?.partial || selection.sel !== uid || selection.agent !== agent || historyPageRequests.has(key)) return;
  const request = {key, uid, agent, entry, cursor: entry?.partial?.cursor, viewRequest};
  // Unlike a gap page, a window reset replaces the complete cached snapshot.
  // An SSE update accepted after this request started must never be rolled back.
  const observed = {msgs: entry.msgs, version: entry.version, end: entry.end, anchor: entry.anchor,
    activity: entry.activity, meta: entry.meta, prompt: entry.prompt};
  const ac = new AbortController(), timer = setTimeout(() => ac.abort(), sync.timing.syncStallMs);
  request.ac = ac;
  historyPageRequests.set(key, request);
  Conversation.gapState({gapReloadBusy:true});
  try {
    const {data, bytes} = await fetchMessages(uid, {agent, windowed: true, signal: ac.signal});
    if (!currentHistoryPage(request)) return;
    if (entry.msgs !== observed.msgs || entry.version !== observed.version
        || entry.end !== observed.end || entry.anchor !== observed.anchor
        || entry.activity !== observed.activity || entry.meta !== observed.meta || entry.prompt !== observed.prompt) {
      throw new Error('实时历史已更新；已保留新内容，请再次重新载入当前历史。');
    }
    if (!data?.reset || !data.meta || !data.version || !Array.isArray(data.messages)) throw new Error('服务端没有返回有效的当前历史窗口。');
    await applyDiff(uid, data, bytes, agent);
  } catch (error) { historyPageFailure(request, button.parentElement.querySelector('.history-gap-load'), error); }
  finally {
    clearTimeout(timer);
    if (currentHistoryPageEntry(request)) Conversation.gapState({gapReloadBusy:false});
    if (historyPageRequests.get(key) === request) historyPageRequests.delete(key);
  }
}

async function loadFullHistory(uid, agent, button) {
  const key = viewKey(uid, agent);
  const old = cache.get(key);
  if (!old?.partial) return;
  requests.inflight?.abort();
  const ac = requests.inflight = new AbortController();
  closeWatch();
  Conversation.gapState({gapDisabled:true,gapLabel:'正在载入完整历史…'});
  progress(0, 0, '下载完整历史');
  try {
    const {data, bytes} = await fetchMessages(uid, {
      agent, signal: ac.signal,
      onProgress: (a, b, detail) => progress(a, b, '下载完整历史', detail),
    });
    cachePut(key, {meta: data.meta, msgs: data.messages, version: data.version,
                   end: data.end, anchor: data.anchor, activity: data.activity, bytes,
                   prompt: data.prompt || null, cli: data.cli ?? null,
                   total: data.message_total, partial: null});
    unread.cursors.set(key, {end: data.end, head: data.version.head, anchor: data.anchor});
    if (selection.sel === uid && selection.agent === agent) {
      await renderSession(data.meta, data.messages, data.activity);
    }
  } catch (e) {
    if (e.name !== 'AbortError') {
      if (selection.sel === uid && selection.agent === agent) Conversation.gapState({gapDisabled:false,gapLabel:'载入失败，点击重试'});
    }
  } finally {
    progressDone();
    const current = cache.get(key);
    if (selection.sel === uid && selection.agent === agent && current?.partial && !sync.watching) {
      watchSession(uid, agent);
    }
  }
}

  return {timing, historyPageRequests, historyPagesEnabled, currentHistoryPage, historyPageFailure, restoreHistoryPageScroll, loadHistoryPage, currentHistoryPageEntry, reloadHistoryWindow, loadFullHistory};
}
