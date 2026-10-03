import {viewKey} from '../../domain/runtime/messages.js'
import {useCacheSettingsStore} from '../../stores/runtime/cache'

export function createMessageCache(pinia, preferences, selection, live, terminalRows) {
  const cache = new Map();
  const settings = useCacheSettingsStore(pinia);
  settings.limitMb = Math.max(0, +preferences.get('cacheMb', 256) || 0);
function queuedAfterTimestamp(uid) {
  const entry = cache.get(viewKey(uid));
  let latest = Date.parse(entry?.activity?.ts || '');
  for (let i = (entry?.msgs?.length || 0) - 1; i >= 0; i--) {
    const at = Date.parse(entry.msgs[i]?.ts || '');
    if (!Number.isFinite(at)) continue;
    latest = Number.isFinite(latest) ? Math.max(latest, at) : at;
    break;
  }
  return Number.isFinite(latest) ? new Date(latest).toISOString() : null;
}

function cacheGet(uid) {
  const e = cache.get(uid);
  if (e) { cache.delete(uid); cache.set(uid, e); }   // 命中即移到队尾
  return e;
}

function cacheEntryUid(key, entry) {
  // 子代理视图的 key 是 uid::agent，但它和主会话共用同一个 tmux 生命周期。
  return entry?.meta?.uid || String(key).split('::', 1)[0];
}

function cacheEntryPinned(key, entry) {
  const uid = cacheEntryUid(key, entry);
  // 详情 DOM 直接由当前缓存生成。若容量整理把正在看的这一份删掉，页面仍
  // 看似正常，却再也没有游标可供 SSE/outbox 补读，最终会留下永久 pending。
  return key === viewKey(selection.sel, selection.agent)
    || live.liveTmux.has(uid)
    || terminalRows().some(x => x.uid === uid);
}

function trimCache() {
  // tmux 对话必须随时切回即见，因此不计入容量、也不参与淘汰。
  // 普通历史对话单独共享用户设置的容量，并延续“至少保留最新一份”的旧行为。
  const evictable = [...cache].filter(([key, entry]) => !cacheEntryPinned(key, entry));
  let total = evictable.reduce((n, [, entry]) => n + (+entry.bytes || 0), 0);
  let remaining = evictable.length;
  for (const [key, entry] of evictable) {
    if (total <= settings.maxBytes || remaining <= 1) break;
    cache.delete(key);
    total -= +entry.bytes || 0;
    remaining--;
  }
}

function cachePut(uid, e) {
  cache.delete(uid);
  cache.set(uid, e);
  trimCache();
}

  function setLimit(value) {
    settings.limitMb = Math.max(0, +value || 0);
    preferences.set('cacheMb', settings.limitMb);
    trimCache();
  }
  return {cache, settings, queuedAfterTimestamp, cacheGet, cacheEntryUid, cacheEntryPinned, trimCache, cachePut, setLimit};
}
