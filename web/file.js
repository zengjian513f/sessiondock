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
  try {
    const uid = context.get('uid'), ref = context.get('ref');
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
        const frame = document.createElement('iframe'); frame.title = '文件预览'; frame.src = target.href; host.append(frame);
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
