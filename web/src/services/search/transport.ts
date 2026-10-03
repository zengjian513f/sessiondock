import type { SearchRow, SearchNode, SearchData } from '../../domain/search/types'
interface SearchProgress {done: number; total: number | null; nodes?: SearchNode[]; total_known?: boolean}
export interface SearchCallbacks {fetch: typeof fetch; allowsSearch(): boolean; appUrl(path: string): string; progress(done: number, total: number | null, nodes?: SearchNode[] | null, totalKnown?: boolean): void; matches(rows: SearchRow[]): void}
export async function fetchSearch(params: URLSearchParams, signal: AbortSignal, callbacks: SearchCallbacks): Promise<{ok: boolean; data: SearchData}> {
  if (!callbacks.allowsSearch()) {
    return {ok: false, data: {error: 'Rust 后端尚未实现全文搜索；当前只能筛选标题和目录。'}};
  }
  params.set('progress', '1');
  const r = await callbacks.fetch(callbacks.appUrl('api/search?' + params), { signal });
  if (!r.ok || !r.headers.get('Content-Type')?.includes('application/x-ndjson')) {
    return { ok: r.ok, data: await r.json() };
  }
  const reader = r.body!.getReader(), dec = new TextDecoder();
  let buf = '', result: SearchData | null = null, error: string | null = null;
  let pendingRows: SearchRow[] = [], progress: SearchProgress | null = null, paintTimer: ReturnType<typeof setTimeout> | null = null;
  const paint = () => {
    paintTimer = null;
    if (signal.aborted) return;
    if (progress) {
      callbacks.progress(progress.done, progress.total, progress.nodes,
        progress.total_known ?? Number.isFinite(progress.total));
      progress = null;
    }
    if (pendingRows.length) {
      callbacks.matches(pendingRows);
      pendingRows = [];
    }
  };
  const schedulePaint = () => {
    // A proxy may coalesce hundreds of NDJSON records into one read. Paint
    // the accumulated results once, not the entire sidebar for every record.
    if (paintTimer === null) paintTimer = setTimeout(paint, 50);
  };
  let sliceStart = performance.now();
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (signal.aborted) return { ok: false, data: {} };
      buf += dec.decode(value || new Uint8Array(), { stream: !done });
      const lines = buf.split('\n');
      buf = done ? '' : lines.pop()!;
      for (const line of lines) {
        // Awaiting an already buffered read only yields to microtasks. Give
        // keyboard input a task turn so Backspace/Escape can actually abort.
        if (performance.now() - sliceStart >= 8) {
          await new Promise(resolve => setTimeout(resolve, 0));
          sliceStart = performance.now();
        }
        if (signal.aborted) return { ok: false, data: {} };
        if (!line) continue;
        const event = JSON.parse(line);
        if (event.type === 'progress') { progress = event; schedulePaint(); }
        else if (event.type === 'matches') {
          for (const row of event.results || []) pendingRows.push(row);
          schedulePaint();
        } else if (event.type === 'result') result = event.data;
        else if (event.type === 'error') error = event.error;
      }
      if (done) break;
    }
  } finally {
    if (paintTimer !== null) clearTimeout(paintTimer);
    paint();
    reader.releaseLock();
  }
  return error ? { ok: false, data: { error } }
    : result ? { ok: true, data: result }
      : { ok: false, data: { error: '搜索响应不完整' } };
}
