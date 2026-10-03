export function createEnvironment(capabilities, selectedNodeIds, newNodeId) {
  const HUB_MODE = document.querySelector('meta[name="sessiondock-mode"]')?.content === 'hub';
  const STORAGE_PREFIX = capabilities.namespace
    || (HUB_MODE ? `sessiondock.hub.${location.pathname}.` : 'sessiondock.');
  const MOBILE = matchMedia('(max-width: 720px)');
  const MEDIUM = matchMedia('(max-width: 1199px)');
  const layoutTier = () => MOBILE.matches ? 'narrow' : MEDIUM.matches ? 'medium' : 'wide';
const APP_BASE = new URL('.', location.href);
// 深链：?sid=<source>:<sid> 或 ?sid=<sid>，打开指定会话（labdesk 的会话台账用它跳过来）。
// 用 CLI 原生会话号而不是 uid —— uid 是会话文件路径的散列，换目录就变。
const DEEP_SID = (new URLSearchParams(location.search).get('sid') || '').trim().slice(0, 128);
const deepNode = () => new URLSearchParams(location.search).get('node') || '';
const appUrl = path => {
  const url = new URL(String(path).replace(/^\//, ''), APP_BASE);
  if (HUB_MODE && !url.searchParams.has('nodes') && /\/api\/(search|trash|trash\/purge)$/.test(url.pathname)) {
    url.searchParams.set('nodes', selectedNodeIds().join(','));
  }
  if (HUB_MODE && /\/api\/term\/(complete-dir|models)$/.test(url.pathname)
      && !url.searchParams.get('node')) url.searchParams.set('node', newNodeId());
  return url.toString();
};
const BUILD_ID = document.querySelector('meta[name="sessiondock-build"]')?.content || '';
// One ephemeral page identity joins HTTP, SSE, terminal and final DOM receipts.
// It intentionally is not persisted: duplicated/restored tabs must remain distinct.
const AUDIT_PAGE_ID = globalThis.crypto?.randomUUID?.()
  || [...globalThis.crypto.getRandomValues(new Uint8Array(16))]
    .map(value => value.toString(16).padStart(2, '0')).join('');

  return {HUB_MODE, STORAGE_PREFIX, MOBILE, MEDIUM, layoutTier, APP_BASE, DEEP_SID, deepNode, appUrl, BUILD_ID, AUDIT_PAGE_ID};
}
