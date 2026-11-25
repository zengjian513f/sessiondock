'use strict';
// Conversation reads stay local; directory management is an optional integration.
(async () => {
  const base = new URL('.', location.href), context = new URLSearchParams(location.search);
  const host = document.getElementById('file-content');
  const integration = globalThis.SessionDockCapabilities?.integration('filedock');
  let path = context.get('path') || '';
  function title(value) {
    document.title = value;
    document.getElementById('file-title').textContent = value.replace(/ · SessionDock$/, '');
    document.querySelector('header').hidden = false;
  }
  function copyPath() {
    if (!path) return;
    const copy = document.getElementById('file-copy');
    copy.hidden = false;
    copy.onclick = async () => {
      try { await navigator.clipboard.writeText(path); copy.textContent = '已复制'; }
      catch { host.textContent = '请复制文件路径：' + path; }
    };
  }
  function unavailable() {
    title('文件浏览器未启用 · SessionDock');
    host.textContent = '未配置外部文件浏览器。会话中的单文件仍可预览和下载。' + (path ? '\n' + path : '');
    copyPath();
  }
  async function json(response) {
    if (!(response.headers.get('Content-Type') || '').includes('application/json')) throw new Error('请登录后刷新页面');
    const value = await response.json();
    if (!response.ok) throw new Error(value.error || '无法读取文件');
    return value;
  }
  function watchTaskOutput(uid, ref, task) {
    title('后台输出 · ' + task + ' · SessionDock');
    document.body.classList.add('task-output');
    const target = new URL('api/session/file', base);
    target.search = new URLSearchParams({uid, ref, agent: context.get('agent') || '', download: '1'});
    const download = document.getElementById('file-download');
    download.href = target.href;
    download.textContent = '下载完整日志';
    download.hidden = false;
    const refresh = document.getElementById('file-retry');
    refresh.hidden = false;
    const follow = document.createElement('input');
    follow.type = 'checkbox';
    follow.checked = true;
    const label = document.createElement('label');
    label.append(follow, '自动刷新');
    refresh.before(label);
    const status = document.createElement('div');
    status.setAttribute('role', 'status');
    status.textContent = '正在读取输出…';
    const pre = document.createElement('pre');
    host.removeAttribute('role');
    host.replaceChildren(status, pre);
    let timer;
    function schedule() {
      clearTimeout(timer);
      if (follow.checked && !document.hidden) timer = setTimeout(read, 3000);
    }
    async function read() {
      if (refresh.disabled) return;
      refresh.disabled = true;
      clearTimeout(timer);
      try {
        const response = await fetch(target, {cache: 'no-store', headers: {Range: 'bytes=-65536'}});
        const range = response.headers.get('Content-Range') || '';
        const empty = response.status === 416 && range === 'bytes */0';
        if (!empty && response.status !== 206) {
          if (response.headers.get('Content-Type')?.includes('application/json')) await json(response);
          await response.body?.cancel();
          throw new Error('无法读取日志，请检查连接或登录状态');
        }
        const bytes = empty ? new Uint8Array() : new Uint8Array(await response.arrayBuffer());
        const clipped = !empty && !range.startsWith('bytes 0-');
        let first = 0;
        while (clipped && (bytes[first] & 0xc0) === 0x80) first++;
        const output = new TextDecoder().decode(bytes.subarray(first));
        const atEnd = pre.scrollHeight - pre.scrollTop - pre.clientHeight <= 2;
        status.textContent = empty ? '暂时没有输出' : clipped ? '显示最近 64 KiB' : '';
        if (pre.textContent !== output) pre.textContent = output;
        if (atEnd) pre.scrollTop = pre.scrollHeight;
      } catch (error) {
        status.textContent = '读取失败 · ' + error.message;
      } finally {
        refresh.disabled = false;
        schedule();
      }
    }
    refresh.onclick = read;
    follow.onchange = () => follow.checked ? read() : clearTimeout(timer);
    document.addEventListener('visibilitychange', () => {
      if (!document.hidden && follow.checked) read();
      else clearTimeout(timer);
    });
    read();
  }
  try {
    const uid = context.get('uid'), ref = context.get('ref');
    const task = context.get('task');
    if (uid && ref && /^task-\d+$/.test(task)) { watchTaskOutput(uid, ref, task); return; }
    if (!integration) {
      // An absolute path alone does not establish a conversation file grant.
      if (!uid || !ref) { unavailable(); return; }
      const target = new URL('api/session/file', base);
      target.search = new URLSearchParams({uid, ref, agent: context.get('agent') || '', mode: 'info'});
      const info = await json(await fetch(target));
      path = info.path || path;
      if (info.kind === 'directory') { unavailable(); return; }
      title((info.name || '文件预览') + ' · SessionDock');
      copyPath();
      const download = document.getElementById('file-download');
      target.searchParams.delete('mode'); target.searchParams.set('download', '1');
      download.href = target.href; download.hidden = false;
      host.textContent = '';
      if (info.preview === 'text') {
        const pre = document.createElement('pre'); pre.textContent = info.text || ''; host.append(pre);
        if (info.truncated) host.append(document.createTextNode('预览已截断；下载可查看完整文件。'));
      } else if (info.preview && info.preview !== 'unsupported') {
        target.searchParams.delete('download'); target.searchParams.set('mode', 'preview');
        if (info.preview.startsWith('audio/')) {
          // A media document inside an iframe inherits the file response's
          // sandbox. Load audio as media in this viewer, preserving file CSP.
          const audio = document.createElement('audio');
          audio.controls = true; audio.preload = 'metadata'; audio.src = target.href;
          audio.style.maxWidth = '100%'; audio.setAttribute('aria-label', '音频预览');
          host.append(audio);
        } else {
          const frame = document.createElement('iframe'); frame.title = '文件预览'; frame.src = target.href; host.append(frame);
        }
      } else host.textContent = '此格式无法预览，请下载后查看。';
      return;
    }
    let node = context.get('node') || (uid || '').match(/^[^:]+:([a-f0-9]{32})~/)?.[1];
    if (!node) node = (await json(await fetch(new URL('api/meta', base)))).node_id;
    if (!node) throw new Error('未找到文件所在机器');
    if (!path) {
      if (!ref) throw new Error('缺少文件路径');
      const result = await json(await fetch(new URL('api/session/resolve-files', base), {
        method: 'POST', headers: {'Content-Type': 'application/json'},
        body: JSON.stringify({uid, agent: context.get('agent') || '', refs: [ref]}),
      }));
      path = result.targets?.find(item => item.ref === ref)?.path || result.resolved?.[ref];
      if (!path) throw new Error(result.errors?.find(item => item.ref === ref)?.error || '无法确定文件的完整路径');
    }
    integration.search = new URLSearchParams({node, path}); integration.hash = location.hash;
    location.replace(integration);
  } catch (error) {
    title('无法打开文件 · SessionDock');
    const retry = document.getElementById('file-retry'); retry.hidden = false; retry.onclick = () => location.reload();
    host.textContent = error.message || '无法打开文件';
    copyPath();
  }
})();
