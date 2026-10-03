import {useMetadataOperationsStore} from '../../stores/runtime/metadata'

export function createMetadataService(pinia, environment, state, messageCache, index, nodes, sync, presentation) {
  const {fetch, appUrl, capabilities} = environment;
  const {catalog, search, selection, sidebar} = state;
  const {cache} = messageCache;
  const {sidebarSessions, indexedSessions} = index;
  const sessionStarred = uid => !!indexedSessions().byUid.get(uid)?.starred;
  const {renderNodes} = nodes;
  const {syncSession} = sync;
  const {renderChips, renderSide, showSessionCount, renderSessionAction, renderForkChainMenu,
    appAlert, paintStarButton, status} = presentation;
  const $ = selector => document.querySelector(selector);
function applySessionStar(uid, starred, starredAt = null) {
  for (const rows of [catalog.sessions, search.results || []]) {
    const row = rows.find(s => s.uid === uid);
    if (!row) continue;
    row.starred = starred;
    if (starredAt) row.starred_at = starredAt;
    else delete row.starred_at;
  }
  for (const entry of cache.values()) {
    if (entry.meta?.uid !== uid) continue;
    entry.meta.starred = starred;
    if (starredAt) entry.meta.starred_at = starredAt;
    else delete entry.meta.starred_at;
  }
}

function applyForkParentVisibility(uid, visible) {
  for (const rows of [catalog.sessions, search.results || []]) {
    const row = rows.find(session => session.uid === uid);
    if (row?.fork_parent) row.fork_parent_visible = visible;
  }
  for (const entry of cache.values()) {
    if (entry.meta?.uid === uid && entry.meta.fork_parent) {
      entry.meta.fork_parent_visible = visible;
    }
  }
}

async function setForkParentVisibility(uids, visible, button = null) {
  const unique = [...new Set(uids)].filter(Boolean);
  if (!unique.length) return {updated: [], errors: []};
  if (button) button.disabled = true;
  try {
    const response = await fetch(appUrl('api/sessions/fork-visibility'), {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({uids: unique, visible}),
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(data.error || `HTTP ${response.status}`);
    for (const row of data.updated || []) {
      applyForkParentVisibility(row.uid, !!row.fork_parent_visible);
    }
    renderNodes();
    renderChips();
    renderSide();
    showSessionCount();
    const selected = catalog.sessions.find(session => session.uid === selection.sel);
    if (selected) renderSessionAction(selected);
    renderForkChainMenu();
    if (data.errors?.length) {
      await appAlert(`父会话显示状态有 ${data.errors.length} 项未保存：${data.errors[0].error}`);
    }
    return data;
  } catch (error) {
    await appAlert('父会话显示状态保存失败: ' + error.message);
    return null;
  } finally {
    if (button) button.disabled = false;
  }
}

function refreshStarPresentation(uid) {
  const side = $('#side');
  const top = side?.scrollTop || 0;
  renderSide();
  if (side) side.scrollTop = top;
  if (selection.sel === uid) paintStarButton($('#a-star'), sessionStarred(uid), sidebar.starBusy.has(uid));
}

async function toggleSessionStar(uid) {
  if (!uid || sidebar.starBusy.has(uid)) return;
  const before = sessionStarred(uid);
  const wanted = !before;
  sidebar.starBusy.add(uid);
  applySessionStar(uid, wanted);
  refreshStarPresentation(uid);
  try {
    const response = await fetch(appUrl('api/session/star'), {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({uid, starred: wanted}),
    });
    const data = await response.json();
    if (!response.ok) throw new Error(data.error || `HTTP ${response.status}`);
    applySessionStar(uid, !!data.starred, data.starred_at || null);
  } catch (error) {
    applySessionStar(uid, before);
    const stat = status;
    if (stat) {
      stat.textContent = ' 星标保存失败';
      stat.classList.add('err');
      setTimeout(() => { stat.classList.remove('err'); showSessionCount(sidebarSessions().length); }, 1800);
    }
    console.error('星标保存失败', error);
  } finally {
    sidebar.starBusy.delete(uid);
    refreshStarPresentation(uid);
  }
}

/* ---------- Claude 时间线固定显示（Rust 只读迁移能力） ----------
 * 只改 SessionDock的显示时间线；不写原生记录，也不给 CLI 发任何回滚信号。
 * 没有这个能力声明的页面，保持原有双 Esc 原生回滚流程。 */
function timelinePinEnabled() {
  return capabilities.config.backend === 'rust'
    && capabilities.config.timeline_pin === true;
}

function timelinePinFailed(message) {
  const stat = status;
  if (!stat) return;
  stat.textContent = ` ${message}`;
  stat.classList.add('err');
  setTimeout(() => { stat.classList.remove('err'); showSessionCount(sidebarSessions().length); }, 2400);
}

const timelinePinBusy = useMetadataOperationsStore(pinia).timelinePinBusy;
async function pinTimeline(uid, target) {
  if (!timelinePinEnabled() || !uid || timelinePinBusy.has(uid)) return false;
  timelinePinBusy.add(uid);
  try {
    const response = await fetch(appUrl('api/session/rewind'), {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({uid, target: target ?? null}),
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(data.error || `HTTP ${response.status}`);
    // 固定/取消固定改变逻辑时间线：即便 JSONL 一个字节没变，服务端也会给
    // reset；用现有增量接口原子替换缓存和 DOM。
    selection.lastSync = 0;
    await syncSession(uid, null);
    return true;
  } catch (error) {
    timelinePinFailed(`${target ? '固定显示失败' : '取消固定失败'}: ${error.message}`);
    console.error('时间线固定失败', error);
    return false;
  } finally {
    timelinePinBusy.delete(uid);
  }
}

  return {sessionStarred, applySessionStar, applyForkParentVisibility, setForkParentVisibility, refreshStarPresentation, toggleSessionStar, timelinePinEnabled, timelinePinFailed, pinTimeline};
}
