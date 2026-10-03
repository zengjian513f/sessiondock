const $ = selector => document.querySelector(selector);

import * as Conversation from '../../migration/conversation'

import * as Overlays from '../../migration/overlays'

import {viewKey} from '../../domain/runtime/messages.js'

export function createMediaRuntime({dom,core,capabilities}) {
const {safeMediaSrc, lazyMediaEnabled, mediaContinuationEnabled, diagnoseMedia,
  reloadMediaSession, forgetMediaDiagnostic, captureView} = Overlays;

function imageHtml(m, inline = false) {
  if (m?.error) {
    const reason = dom().esc(String(m.error.message || '图片不可用'));
    return `<span class="media-error${inline ? ' inline' : ''}" role="status">图片不可用：${reason}</span>`;
  }
  const src = safeMediaSrc(m?.src);
  if (!src) return '';
  const alt = dom().esc(m.alt || '图片');
  const w = Number.isFinite(+m.width) && +m.width > 0 ? ` width="${Math.round(+m.width)}"` : '';
  const h = Number.isFinite(+m.height) && +m.height > 0 ? ` height="${Math.round(+m.height)}"` : '';
  const lazy = lazyMediaEnabled() && m.lazy === true;
  const tracking = lazy ? ` data-media-lazy="true" data-media-path="${dom().esc(m.src)}"` : '';
  const html = `<a class="media-link${inline ? ' inline' : ''}" href="${dom().esc(src)}" target="_blank" rel="noopener noreferrer">
    <img loading="lazy" decoding="async" referrerpolicy="no-referrer" src="${dom().esc(src)}" alt="${alt}"${w}${h}${tracking}>
    ${inline ? '' : `<span>${alt}</span>`}
  </a>`;
  return lazy ? `<span class="media-load${inline ? ' inline' : ''}">${html}</span>` : html;
}

function mediaMoreInfo(more) {
  if (!more || typeof more !== 'object' || Array.isArray(more)) return null;
  const {remaining, total, cursor} = more;
  if (!Number.isSafeInteger(remaining) || remaining <= 0 || !Number.isSafeInteger(total) || total < remaining) return null;
  if (cursor !== null && !(typeof cursor === 'string' && /^[0-9a-f]{32}$/.test(cursor))) return null;
  return {remaining, total, cursor};
}

const mediaPageRequests = Conversation.requests.mediaPageRequests;

function currentMediaPage(request) {
  return core.state.selection.sel === request.uid && core.state.selection.agent === request.agent
    && core.open.requests.inflight === request.viewRequest && core.cache.cache.get(request.key) === request.entry
    && request.message.media_more?.cursor === request.cursor;
}

const validateMediaPage = Conversation.pages.validateMediaPage;

async function fetchMediaPage(uid, agent, cursor, signal) {
  const query = new URLSearchParams({cursor});
  if (agent) query.set('agent', agent);
  const response = await core.network.fetch(core.environment.appUrl(`api/messages/${encodeURIComponent(uid)}/media-page?${query}`),
    {signal, cache: 'no-store'});
  if (!response.ok) {
    const detail = await response.json().catch(() => null);
    const error = new Error(detail?.error || `HTTP ${response.status}`);
    error.status = response.status;
    throw error;
  }
  const reader = response.body.getReader(), chunks = [];
  let bytes = 0;
  try {
    for (;;) {
      const {done, value} = await reader.read();
      if (done) break;
      bytes += value.length;
      if (bytes > 1024 * 1024) throw new Error('图片分页响应超过浏览器读取预算。');
      chunks.push(value);
    }
  } catch (error) { await reader.cancel().catch(() => {}); throw error; }
  const buffer = new Uint8Array(bytes);
  let offset = 0;
  for (const chunk of chunks) { buffer.set(chunk, offset); offset += chunk.length; }
  return {data: JSON.parse(new TextDecoder().decode(buffer)), bytes};
}

function mediaPageFailure(request, button, error) {
  if (!currentMediaPage(request)) return;
  Conversation.mediaState(request.cursor,{busy:false,error:`已加载的图片保持不变${error.status ? `（HTTP ${error.status}）` : ''}：${error.message || '图片分页读取失败。'}`});
}

async function loadMediaContinuation(uid, agent, cursor, button) {
  agent = agent || null;
  const key = viewKey(uid, agent), entry = core.cache.cache.get(key);
  if (!mediaContinuationEnabled() || !entry?.msgs || !/^[0-9a-f]{32}$/.test(cursor || '')) return;
  const message = entry.msgs.find(m => m?.media_more?.cursor === cursor);
  if (!message || !mediaMoreInfo(message.media_more)) return;
  const previous = mediaPageRequests.get(cursor);
  if (previous && currentMediaPage(previous)) return;
  if (previous) { previous.ac?.abort(); mediaPageRequests.delete(cursor); }
  const request = {key, uid, agent, entry, message, cursor, viewRequest: core.open.requests.inflight};
  if (!currentMediaPage(request)) return;
  const ac = new AbortController();
  request.ac = ac;
  const timer = setTimeout(() => ac.abort(), core.sync.timing.syncStallMs);
  mediaPageRequests.set(cursor, request);
  Conversation.mediaState(cursor,{busy:true,error:''});
  try {
    const {data} = await fetchMediaPage(uid, agent, cursor, ac.signal);
    if (!currentMediaPage(request)) return;
    const page = validateMediaPage(data, message.media_more, cursor);
    // Only this message changes. Live cursor fields, message order and any
    // SSE tail accepted while HTTP was pending stay exactly as observed.
    message.media = [...(message.media || []), ...data.media];
    if (page.remaining) message.media_more = {remaining: page.remaining, total: page.total, cursor: page.next};
    else delete message.media_more;
    Conversation.mediaState(cursor, {busy:false,error:''});
    Conversation.updateMedia();
  } catch (error) { mediaPageFailure(request, button, error); }
  finally {
    clearTimeout(timer);
    if (mediaPageRequests.get(cursor) === request) mediaPageRequests.delete(cursor);
  }
}
function start(){
Overlays.mountMedia({fetch:(...args)=>core.network.fetch(...args),appUrl:core.environment.appUrl, capabilities:capabilities, hub:core.environment.HUB_MODE,
  viewKey:viewKey, cache:core.cache.cache, historyPageRequests:core.history.historyPageRequests, stallMs:core.sync.timing.syncStallMs, fetchMessages:(...args)=>core.reader.fetchMessages(...args), applyDiff:(...args)=>core.diff.applyDiff(...args),
  getView: () => ({uid:core.state.selection.sel, agent:core.state.selection.agent, request:core.open.requests.inflight}), contains: element => !!$('#msgs')?.contains(element)});
document.addEventListener('click', event => {
  const button = event.target?.closest?.('.media-more');
  if (!button || !button.closest('[data-conversation-inner]') || button.disabled || !mediaContinuationEnabled() || !$('#msgs')?.contains(button)) return;
  event.preventDefault();
  loadMediaContinuation(core.state.selection.sel, core.state.selection.agent, button.dataset.mediaCursor, button);
});
}
return {get diagnosticActive(){return Overlays.mediaState.diagnosticActive},safeMediaSrc,lazyMediaEnabled,mediaContinuationEnabled,diagnoseMedia,reloadMediaSession,forgetMediaDiagnostic,captureView,imageHtml,mediaMoreInfo,mediaPageRequests,currentMediaPage,validateMediaPage,fetchMediaPage,mediaPageFailure,loadMediaContinuation,start};
}
