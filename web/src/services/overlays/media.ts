import { createInlineMedia } from './inline-media'
export interface MediaDiagnostic {status: number; message: string}
export interface MediaContext {uid: string; agent: string | null; request: unknown}
export interface MediaView extends MediaContext {current(): boolean}
export interface MediaEntry { [field: string]: unknown }
export interface MediaDependencies {
 getView(): MediaContext
 contains(element: Node): boolean
 appUrl(path: string): string
 capabilities: {config: Readonly<Record<string, unknown>>; allows(name: string): boolean}
 hub: boolean
 viewKey(uid: string, agent: string | null): string
 cache: Map<string, MediaEntry>
 historyPageRequests: Map<string, {key: string; entry: MediaEntry; ac: AbortController}>
 stallMs: number
 fetchMessages(uid: string, options: {agent: string | null; windowed: boolean; signal: AbortSignal}): Promise<{data: {reset?: boolean; meta?: unknown; version?: unknown; messages?: unknown[]}; bytes: number}>
 applyDiff(uid: string, data: unknown, bytes: number, agent: string | null): Promise<unknown>
}
export function createMedia(deps: MediaDependencies) {
function captureView(element: HTMLElement, currentGeneration: () => number): MediaView {
 const captured = deps.getView()
 const generation = currentGeneration()
 return {...captured, current() {
  const selected = deps.getView()
  return element.isConnected && deps.contains(element) && currentGeneration() === generation
   && selected.uid === captured.uid && selected.agent === captured.agent && selected.request === captured.request
 }}
}
function safeMediaSrc(src: unknown) {
  const source = String(src || '');
  if (/^\/api\/media\/[0-9a-f]{32}$/.test(source)) return deps.appUrl(source);
  if (deps.capabilities.config.backend === 'rust' && deps.capabilities.config.media_lazy === true) return '';
  if (deps.hub && /^\/api\/nodes\/[0-9a-f]{32}\/api\/media\/[0-9a-f]{32}$/.test(source)) return deps.appUrl(source);
  // Native history is untrusted: Rust's local media capability does not grant
  // permission for the browser to contact URLs mentioned in that history.
  if (!deps.capabilities.allows('media_remote')) return '';
  if (!/^https?:\/\//i.test(source)) return '';
  try {
    const u = new URL(source);
    return (u.protocol === 'http:' || u.protocol === 'https:') ? u.href : '';
  } catch { return ''; }
}

function lazyMediaEnabled() {
  return deps.capabilities.config.backend === 'rust'
    && deps.capabilities.config.media_lazy === true;
}

function mediaContinuationEnabled() {
  return deps.capabilities.config.backend === 'rust'
    && deps.capabilities.config.media_continuation === true;
}

// Error-only diagnostics never materialize an image body. Completed results
// are bounded/deduplicated; in-flight work owns its slot even across eviction.
const mediaDiagnostics = new Map<string, Promise<MediaDiagnostic>>();
let mediaDiagnosticActive = 0;

async function diagnoseMedia(path: string): Promise<MediaDiagnostic> {
  if (!lazyMediaEnabled() || !/^\/api\/media\/[0-9a-f]{32}$/.test(path)) {
    return {status: 0, message: '图片地址不可用。'};
  }
  if (mediaDiagnostics.has(path)) return mediaDiagnostics.get(path)!;
  if (mediaDiagnosticActive >= 4) return {status: 0, message: '图片错误诊断繁忙，请手动重试。'};
  mediaDiagnosticActive++;
  const ac = new AbortController(), timer = setTimeout(() => ac.abort(), 5000);
  const pending = (async () => {
    try {
      const response = await fetch(safeMediaSrc(path), {signal: ac.signal, cache: 'no-store',
        redirect: 'error', headers: {Accept: 'application/json'}});
      if (response.ok) {
        await response.body?.cancel();
        return {status: response.status, message: '图片读取已恢复或浏览器无法解码；请手动重试图片。'};
      }
      const result = {status: response.status, message: '图片读取失败。'};
      if (Number(response.headers.get('content-length')) > 4096) {
        await response.body?.cancel();
        return result;
      }
      if (!/^application\/json(?:;|$)/i.test(response.headers.get('content-type') || '')) {
        await response.body?.cancel();
        return result;
      }
      const reader = response.body?.getReader();
      if (!reader) return result;
      const chunks = [];
      let bytes = 0;
      try {
        for (;;) {
          const {value, done} = await reader.read();
          if (done) break;
          bytes += value.byteLength;
          if (bytes > 4096) { await reader.cancel(); return result; }
          chunks.push(value);
        }
        const buffer = new Uint8Array(bytes);
        let offset = 0;
        for (const chunk of chunks) { buffer.set(chunk, offset); offset += chunk.byteLength; }
        const detail = JSON.parse(new TextDecoder().decode(buffer));
        if (typeof detail?.error === 'string') result.message = detail.error.slice(0, 512);
      } finally { reader.releaseLock(); }
      return result;
    } catch (error) {
      return {status: 0, message: (error as Error).name === 'AbortError' ? '图片错误诊断超时，请手动重试。' : '图片请求失败，请检查连接后手动重试。'};
    } finally { clearTimeout(timer); mediaDiagnosticActive--; }
  })();
  mediaDiagnostics.set(path, pending);
  while (mediaDiagnostics.size > 128) mediaDiagnostics.delete(mediaDiagnostics.keys().next().value!);
  return pending;
}

async function reloadMediaSession(view: MediaView, button: {disabled: boolean}, notice: {textContent: string}) {
  if (!view.current()) return;
  const key = deps.viewKey(view.uid, view.agent), entry = deps.cache.get(key);
  if (!entry || deps.historyPageRequests.has(key)) {
    notice.textContent = '当前历史请求尚未结束，请稍后手动重新载入。';
    return;
  }
  const observed = Object.fromEntries(['msgs','version','end','anchor','activity','meta','prompt','partial']
    .map(field => [field, entry[field]]));
  const ac = new AbortController(), timer = setTimeout(() => ac.abort(), deps.stallMs);
  const request = {key, entry, ac};
  deps.historyPageRequests.set(key, request);
  button.disabled = true;
  try {
    const {data, bytes} = await deps.fetchMessages(view.uid, {agent: view.agent, windowed: true, signal: ac.signal});
    if (!view.current() || deps.cache.get(key) !== entry) return;
    if (Object.entries(observed).some(([field, value]) => entry[field] !== value)) {
      throw new Error('实时历史已更新；已保留新内容，请再次手动重新载入。');
    }
    if (!data?.reset || !data.meta || !data.version || !Array.isArray(data.messages)) {
      throw new Error('未收到有效历史窗口；当前快照保持不变。');
    }
    await deps.applyDiff(view.uid, data, bytes, view.agent);
  } catch (error) {
    if (view.current()) notice.textContent = `重新载入失败：${(error as Error).message || '请求未完成'}。当前快照保持不变。`;
  } finally {
    clearTimeout(timer);
    button.disabled = false;
    if (deps.historyPageRequests.get(key) === request) deps.historyPageRequests.delete(key);
  }
}


function forgetMediaDiagnostic(path: string) {mediaDiagnostics.delete(path)}
const inline = createInlineMedia({safeMediaSrc, lazyMediaEnabled, captureView, diagnoseMedia, reloadMediaSession, forgetMediaDiagnostic}, deps.contains)
return {safeMediaSrc, lazyMediaEnabled, mediaContinuationEnabled, diagnoseMedia, reloadMediaSession, forgetMediaDiagnostic, captureView,
 startInlineMedia: inline.start, dispose: inline.dispose, get diagnosticActive() {return mediaDiagnosticActive}}
}
export type MediaController = ReturnType<typeof createMedia>
