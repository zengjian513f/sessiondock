'use strict';

// Search input, request cancellation and streamed result rendering share one
// controller. The page supplies its view model and rendering adapters; request
// generations, the debounce timer and AbortController remain private here.
globalThis.SessionDockSearch = Object.freeze({
  create({state: S, select: $, capabilities: SessionDockCapabilities, store,
    hub: HUB_MODE, sources: SOURCES, nodes: Nodes, appUrl, esc,
    resetRegexSearch, reTerm, renderSide, showSessionCount, sidebarMatchCount,
    renderNodes, selectedNodeIds, applyNodeState}) {
    $('#q').oninput = e => {
      cancelSearch();
      S.term = e.target.value.trim();
      searchInputTimer = setTimeout(() => { searchInputTimer = null; renderSide(); }, 120);
    };

    $('#q').onkeydown = async e => {
      if (e.key === 'Escape') { cancelSearch(true); showSessionCount(); renderSide(); return; }
      if (e.key !== 'Enter') return;
      runSearch();
    };

    let searchSeq = 0, searchRun = 0, searchAbort = null, searchInputTimer = null;

    function cancelSearch(clearQuery = false) {
      clearTimeout(searchInputTimer);
      searchInputTimer = null;
      resetRegexSearch();
      ++searchRun;
      searchAbort?.abort();
      searchAbort = null;
      searchProgressDone();
      S.results = null;
      S.searchClosed.clear();
      S.searchNestClosed.clear();
      if (clearQuery) { S.term = ''; $('#q').value = ''; }
      $('#stat').textContent = ''; $('#stat').classList.remove('err');
      if (HUB_MODE) { Nodes.errors.delete('search'); renderNodes(); }
    }

    function searchProgress(done, total, nodes = null, totalKnown = total !== null) {
      const box = $('#search-progress');
      const known = totalKnown && total > 0;
      const percent = known ? Math.min(100, Math.floor(done / total * 100)) : 0;
      box.classList.add('on');
      box.querySelector('b').textContent = known ? `${done} / ${total} 个会话 · ${percent}%`
        : done ? `已扫描 ${done} 个会话` : totalKnown ? '已扫描 0 个会话' : '正在读取会话数量…';
      const track = box.querySelector('.search-progress-track');
      track.hidden = !known;
      if (known) track.setAttribute('aria-valuenow', String(percent));
      else track.removeAttribute('aria-valuenow');
      track.setAttribute('aria-valuetext', box.querySelector('b').textContent);
      box.querySelector('i').style.width = known ? `${Math.min(100, done / total * 100)}%` : '0%';
      const rows = box.querySelector('.search-progress-nodes');
      if (nodes) rows.innerHTML = nodes.map(node => {
        const count = node.total !== null ? `${node.done} / ${node.total}` : `已扫描 ${node.done}`;
        const status = {preparing: '准备中', offline: '离线跳过', error: '搜索失败',
          limited: `${count} · 达到结果上限`, done: `${count} · 已完成`, scanning: `${count} 个会话`}[node.state] || count;
        return `<div class="search-progress-node" data-state="${esc(node.state)}"><span>${esc(node.name)}</span><span>${esc(status)}</span></div>`;
      }).join('');
    }

    function searchProgressDone() {
      const box = $('#search-progress');
      box.classList.remove('on');
      box.querySelector('i').style.width = '0%';
      box.querySelector('.search-progress-nodes').replaceChildren();
    }

    $('#search-cancel').onclick = () => { cancelSearch(true); showSessionCount(); renderSide(); };

    function showSearchMatches(rows) {
      const found = new Map((S.results || []).map(row => [row.uid, row]));
      for (const row of rows) found.set(row.uid, row);
      S.results = [...found.values()].sort((a, b) => String(b.updated).localeCompare(String(a.updated))
        || b.uid.localeCompare(a.uid));
      $('#stat').textContent = ` 已找到 ${sidebarMatchCount(S.results)} 个会话，继续搜索…`;
      renderSide();
    }

    async function fetchSearch(params, signal) {
      if (!SessionDockCapabilities.allows('search')) {
        return {ok: false, data: {error: 'Rust 后端尚未实现全文搜索；当前只能筛选标题和目录。'}};
      }
      params.set('progress', '1');
      const r = await fetch(appUrl('api/search?' + params), { signal });
      if (!r.ok || !r.headers.get('Content-Type')?.includes('application/x-ndjson')) {
        return { ok: r.ok, data: await r.json() };
      }
      const reader = r.body.getReader(), dec = new TextDecoder();
      let buf = '', result = null, error = null;
      let pendingRows = [], progress = null, paintTimer = null;
      const paint = () => {
        paintTimer = null;
        if (signal.aborted) return;
        if (progress) {
          searchProgress(progress.done, progress.total, progress.nodes,
            progress.total_known ?? Number.isFinite(progress.total));
          progress = null;
        }
        if (pendingRows.length) {
          showSearchMatches(pendingRows);
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
          buf = done ? '' : lines.pop();
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
        clearTimeout(paintTimer);
        paint();
        reader.releaseLock();
      }
      return error ? { ok: false, data: { error } }
        : result ? { ok: true, data: result }
          : { ok: false, data: { error: '搜索响应不完整' } };
    }

    async function runSearch() {
      const q = $('#q').value.trim();
      cancelSearch();
      S.term = q;
      const run = searchRun;
      if (!q) { S.results = null; showSessionCount(); renderSide(); return; }
      if (!SessionDockCapabilities.allows('search')) {
        renderSide();
        $('#stat').textContent = ' Rust 后端尚未实现全文搜索；当前只筛选标题和目录。';
        $('#stat').classList.add('err');
        $('#stat').dataset.seq = ++searchSeq;
        return;
      }
      if (S.opts.regex && !reTerm(false)) {   // 本地先验一次, 省掉一次全盘扫描
        S.results = [];
        $('#stat').textContent = ' 正则无效';
        $('#stat').classList.add('err');
        renderSide();
        $('#stat').dataset.seq = ++searchSeq;
        return;
      }
      const p = new URLSearchParams({ q });
      if (!S.opts.regex) p.set('mode', S.opts.mode === 'any' ? 'any' : 'all');
      if (HUB_MODE) p.set('source', Object.keys(SOURCES).filter(x => !S.off.has(x)).join(','));
      for (const k of ['case', 'word', 'regex']) if (S.opts[k]) p.set(k, '1');
      const ac = searchAbort = new AbortController();
      S.results = [];
      $('#stat').textContent = ' 正在搜索…';
      renderSide();
      if (HUB_MODE) { Nodes.errors.delete('search'); renderNodes(); }
      const searchNodes = HUB_MODE ? Nodes.list.filter(node => selectedNodeIds().includes(node.id)).map(node => ({
        id: node.id, name: node.name, done: 0, total: node.online === false ? 0 : null,
        state: node.online === false ? 'offline' : 'preparing',
      })) : null;
      searchProgress(0, null, searchNodes, false);
      let response;
      try {
        response = await fetchSearch(p, ac.signal);
      } catch (e) {
        if (e.name === 'AbortError') return;
        response = { ok: false, data: { error: e.message || '搜索失败' } };
      }
      if (run !== searchRun) return;
      searchAbort = null;
      searchProgressDone();
      const { ok, data: d } = response;
      applyNodeState(d, 'search');
      if (!ok) {                       // 兜底: 前端漏判的非法模式或网络失败
        $('#stat').textContent = ` 已找到 ${S.results?.length || 0} 个会话；` + (d.error || '搜索失败');
        $('#stat').classList.add('err');
      } else {
        $('#stat').classList.remove('err');
        S.results = d.results;
        $('#stat').textContent = d.truncated
          ? ` 命中超过 ${sidebarMatchCount(d.results)} 个会话（已截断，请细化条件）`
          : ` 全文命中 ${sidebarMatchCount(d.results)} 个会话`;
        if (d.partial) {
          const offline = (d.errors || []).filter(e => d.nodes?.some(n => n.id === e.node_id && n.online === false));
          const failed = (d.errors || []).filter(e => !offline.includes(e));
          if (offline.length) $('#stat').textContent += `（${offline.map(e => e.name).join('、')} 离线，未搜索）`;
          if (failed.length) $('#stat').textContent += `（${failed.map(e => e.name).join('、')} 搜索失败，结果不完整）`;
        }
      }
      renderSide();
      // 全文搜索只筛左侧列表；右侧会话的内容、滚动位置和展开状态保持原样。
      $('#stat').dataset.seq = ++searchSeq;              // 供测试判定"这一轮搜索已结束"
    }

    $('#opts').onclick = e => {
      const b = e.target.closest('button[data-o]');
      if (!b) return;
      const k = b.dataset.o;
      S.opts[k] = !S.opts[k];
      store.set('opts', S.opts);
      renderOpts();
      if (S.results) runSearch(); else renderSide();
    };

    $('#search-mode').onclick = e => {
      const b = e.target.closest('button');
      if (!b || b.disabled) return;
      S.opts.mode = S.opts.mode === 'any' ? 'all' : 'any';
      store.set('opts', S.opts);
      renderOpts();
      if (S.results) runSearch(); else renderSide();
    };

    function renderOpts() {
      for (const b of $('#opts').querySelectorAll('button[data-o]')) {
        b.classList.toggle('on', !!S.opts[b.dataset.o]);
        b.setAttribute('aria-pressed', String(!!S.opts[b.dataset.o]));
      }
      const mode = $('#search-mode-toggle'), any = S.opts.mode === 'any';
      mode.textContent = any ? 'OR' : 'AND';
      mode.disabled = !!S.opts.regex;
      mode.title = S.opts.regex ? '正则模式使用整段表达式，不使用 AND / OR'
        : any ? '任一词（OR），点击切换为全部词（AND）'
              : '全部词（AND），点击切换为任一词（OR）；关键词可在不同消息中';
      mode.setAttribute('aria-label', mode.title);
      $('#q').placeholder = S.opts.regex ? '正则搜索…  Enter 搜索对话正文'
                                         : '搜索… Enter 搜正文';
    }

    return Object.freeze({cancel: cancelSearch, run: runSearch, renderOptions: renderOpts});
  },
});
