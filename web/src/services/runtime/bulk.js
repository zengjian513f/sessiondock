
import {STOP_STAGE_TEXT} from '../../domain/session-ui/stop-status'
import {mountDeleted} from '../../migration/session-ui'
import {usePickOperationsStore} from '../../stores/runtime/picking'

// Bulk mutation orchestration and progress live outside sidebar view callbacks.
export function createBulkOperations(pinia, environment, workspace, services, sessionUi, terminal, presentation) {
  const {capabilities, HUB_MODE, fetch, appUrl, store} = environment;
  const {selection, catalog, live, sidebar, search} = workspace;
  const {post, index, pending, sync, groups, live: liveService} = services;
  const {sidebarSessions, indexedSessions} = index;
  const {forgetDeletedReceipts, pendingTmuxSessions} = pending;
  const {closeWatch, watchSession} = sync;
  const {refreshLive} = liveService;
  const {discardPendingSession, loadTermList} = terminal;
  const {sessionStopCapable, sessionStoppable,
    showSessionStopNotice, trashLocationNote, opencodeDeleteNote, trashCapable, confirmForceDelete, requestSessionStop, openTrash} = sessionUi;
  const {appAlert, appConfirm, renderSide, renderChips,
    updatePick, ensureSidebarVue, showMobileList,
    visible, ensureConsolePlaceholder, status, refreshSidebarRows, updateGroups, currentGroups, auditDetailRendered, sessionTurn, paintLive} = presentation;
  const $ = selector => document.querySelector(selector);
const state = usePickOperationsStore(pinia);
const sessionPickable = session => !session.fork_parent;

function sessionStopConcurrency() {
  const value = Number(store.get('stopConcurrency', 6));
  return [1, 2, 4, 6, 8, 12, 16].includes(value) ? value : 6;
}

function pickedStopTargets() {
  return sidebarSessions().filter(s => state.picked.has(s.uid) && sessionPickable(s)
    && (s.pending ? sessionStopCapable() && !!s.record_id && !!s.instance_id
      && !s.stale && !['exited', 'failed'].includes(s.state) && s.running !== false
      : sessionStoppable(s.uid)));
}

/** 列表会被 SSE/轮询整份重画，选择集合里只保留仍然存在且可删的会话。 */
function syncPickedSessions() {
  if (!sidebar.picking) {
    state.picked.clear();
    return state.picked;
  }
  const alive = new Set(sidebarSessions().filter(sessionPickable).map(s => s.uid));
  for (const uid of [...state.picked]) if (!alive.has(uid)) state.picked.delete(uid);
  return state.picked;
}

function setPicking(on) {
  if (state.stopBusy) return;
  state.stopProgress = null; sidebar.picking = !!on;
  if (!sidebar.picking) state.picked.clear();
  if (sidebar.picking) {sidebar.nestAttach = ''; sidebar.nestAttachUids = [];}
  renderPickBar(); renderSide();
}

function toggleSessionPick(uid) {
  if (state.stopBusy) return;
  state.stopProgress = null;
  state.picked.has(uid) ? state.picked.delete(uid) : state.picked.add(uid);
  renderPickBar();
}

/** 整组一起勾/取消：组内还有没选中的就补齐，已经全选才清空。 */
function toggleGroupPick(uids, group) {
  if (state.stopBusy) return;
  state.stopProgress = null;
  const all = uids.length && uids.every(uid => state.picked.has(uid));
  for (const uid of uids) all ? state.picked.delete(uid) : state.picked.add(uid);
  renderPickBar();
}

/** 多选模式下按住鼠标左键划过一片：按下的那条原来没选中就整片选中，原来已选中
 *  就整片取消；划回去恢复按下前的状态，划到列表上下边缘自动滚动。只动了一条时
 *  仍走普通点击。 */
const gesture = {drag: null, swallowClick: false};
function pickDragRows() {
  return [...$('#side').querySelectorAll('.item[data-uid]')]
    .filter(row => row.querySelector('.item-pick') && row.getClientRects().length);
}
function pickDragTo(row) {
  const uid = row?.dataset.uid;
  if (!gesture.drag || !uid || uid === gesture.drag.last) return;
  const rows = pickDragRows();
  const from = rows.findIndex(r => r.dataset.uid === gesture.drag.anchor), to = rows.indexOf(row);
  if (from < 0 || to < 0) return;
  gesture.drag.last = uid;
  gesture.drag.moved = true;
  state.stopProgress = null;
  state.picked.clear();
  for (const kept of gesture.drag.before) state.picked.add(kept);
  for (let i = Math.min(from, to); i <= Math.max(from, to); i++) {
    gesture.drag.on ? state.picked.add(rows[i].dataset.uid) : state.picked.delete(rows[i].dataset.uid);
  }
  renderPickBar();
}
/** 指针所在高度上可见的那一行；落在组标题、间隙或列表外时取纵向最近的可见行。 */
function pickDragAt(y) {
  const side = $('#side').getBoundingClientRect();
  let best = null, gap = Infinity;
  for (const row of pickDragRows()) {
    const box = row.getBoundingClientRect();
    if (box.bottom <= side.top || box.top >= side.bottom) continue;
    const distance = y < box.top ? box.top - y : y > box.bottom ? y - box.bottom : 0;
    if (distance < gap) { best = row; gap = distance; }
    if (!distance) break;
  }
  return best;
}
function pickDragScroll() {
  if (!gesture.drag) return;
  const side = $('#side'), box = side.getBoundingClientRect(), edge = 36;
  const over = gesture.drag.y < box.top + edge ? gesture.drag.y - box.top - edge
    : gesture.drag.y > box.bottom - edge ? gesture.drag.y - box.bottom + edge : 0;
  if (over) {
    side.scrollTop += Math.max(-24, Math.min(24, Math.round(over / 2)));
    pickDragTo(pickDragAt(gesture.drag.y));
  }
  gesture.drag.frame = requestAnimationFrame(pickDragScroll);
}
function endPickDrag() {
  if (!gesture.drag) return;
  cancelAnimationFrame(gesture.drag.frame);
  if (gesture.drag.moved) {
    // 松手处的 click 不能再把按下那条切回去。
    gesture.swallowClick = true;
    setTimeout(() => { gesture.swallowClick = false; }, 0);
  }
  gesture.drag = null;
}
function sidebarGestureMousedown1(event) {
  if (event.button !== 0 || event.shiftKey || event.ctrlKey || event.metaKey || event.altKey) return;
  if (!sidebar.picking || sidebar.nestAttach || state.stopBusy) return;
  const row = event.target.closest('.item[data-uid]');
  if (!row?.querySelector('.item-pick') || event.target.closest('.item-star, .nest-caret')) return;
  event.preventDefault();   // 划选不拉出文字选区
  endPickDrag();
  gesture.drag = {anchor: row.dataset.uid, last: row.dataset.uid, on: !state.picked.has(row.dataset.uid),
              before: new Set(state.picked), moved: false, y: event.clientY};
  gesture.drag.frame = requestAnimationFrame(pickDragScroll);
}
addEventListener('mousemove', event => {
  if (!gesture.drag) return;
  if (!(event.buttons & 1)) { endPickDrag(); return; }
  gesture.drag.y = event.clientY;
  pickDragTo(pickDragAt(event.clientY));
});
addEventListener('mouseup', event => { if (event.button === 0) endPickDrag(); });
addEventListener('blur', endPickDrag);
// 多选时列表里不拉文字选区（含触屏长按）。不用 #side.picking 的 CSS：切换它要为
// 上千行重算样式，进出多选会多卡一两百毫秒。
function sidebarGestureSelectstart1(event) {
  if (sidebar.picking) event.preventDefault();
}
function sidebarGestureClick1(event) {
  if (!gesture.swallowClick) return;
  gesture.swallowClick = false;
  event.stopPropagation();
  event.preventDefault();
}

function pickAllVisible() {
  if (state.stopBusy) return;
  state.stopProgress = null;
  const rows = visible().filter(sessionPickable);
  const all = rows.length && rows.every(s => state.picked.has(s.uid));
  state.picked.clear();
  if (!all) rows.forEach(s => state.picked.add(s.uid));
  const side = $('#side'), top = side.scrollTop;
  renderSide();
  side.scrollTop = top;              // 全选不该把列表弹回顶部
  renderPickBar();
}

function renderPickBar() {
  ensureSidebarVue();
  const attaching = !!sidebar.nestAttach, picked = sidebar.picking ? state.picked.size : 0;
  const row = attaching ? sidebarSessions().find(s => s.uid === sidebar.nestAttach) : null;
  const pending = pendingTmuxSessions().filter(s => state.picked.has(s.uid)).length;
  const action = pending ? (pending === picked ? '丢弃' : '删除 / 丢弃') : '删除';
  const attachable = sidebar.picking ? pickedNestable().length : 0;
  const stoppable = sidebar.picking ? pickedStopTargets().length : 0, progress = state.stopProgress;
  const rows = sidebar.picking ? visible().filter(sessionPickable) : [];
  updatePick({hidden: !sidebar.picking && !attaching, attaching,
    label: attaching ? (sidebar.nestAttachUids.length > 1 ? `点击要附属的会话（当前：${sidebar.nestAttachUids.length} 个会话）` : row ? `点击要附属的会话（当前：${row.title || sidebar.nestAttach}）` : '点击要附属的会话') : picked ? `已选 ${picked} 项` : '点会话行勾选',
    groupHidden: attaching || !groups?.available,
    groupDisabled: !!groups?.busy || ![...state.picked].some(uid => {const row = indexedSessions().byUid.get(uid); return row && !row.pending;}),
    allDisabled: !rows.length || state.stopBusy, allLabel: rows.length && rows.every(s => state.picked.has(s.uid)) ? '全不选' : '全选',
    cancelDisabled: state.stopBusy, deleteLabel: picked ? `${action} (${picked})` : action,
    deleteDisabled: !picked || state.deleteBusy || state.stopBusy,
    attachHidden: attaching || !capabilities.allows('metadata'), attachLabel: attachable ? `附属到… (${attachable})` : '附属到…',
    attachDisabled: !attachable || state.deleteBusy || state.stopBusy,
    stopLabel: progress ? `已停止 ${progress.stopped}/${progress.total}` : stoppable ? `停止 (${stoppable})` : '停止',
    stopDisabled: !stoppable || state.deleteBusy || state.stopBusy, stopBusy: state.stopBusy,
    stopTitle: progress ? `已返回 ${progress.settled}/${progress.total}，失败 ${progress.failed}，未确认 ${progress.uncertain}` : '停止选中的运行中会话',
    detailsHidden: attaching || !sidebar.picking || !progress?.details.length,
    summary: progress ? [progress.failed ? `失败 ${progress.failed}` : '', progress.uncertain ? `未确认 ${progress.uncertain}` : '', progress.refreshError ? '状态刷新失败' : ''].filter(Boolean).join('，') : '',
    errors: progress ? progress.details.join('\n') : ''});
  refreshSidebarRows();
  updateGroups(currentGroups().map(group => {
    const picked = group.pickUids.filter(uid => state.picked.has(uid)).length;
    return {...group, picked: !!group.pickUids.length && picked === group.pickUids.length, indeterminate: picked > 0 && picked < group.pickUids.length};
  }));
}

async function deleteSessions(uids, button = null) {
  if (!uids.length || state.deleteBusy || state.stopBusy) return null;
  const pending = pendingTmuxSessions().filter(s => uids.includes(s.uid));
  const pendingIds = new Set(pending.map(s => s.uid));
  const recorded = uids.filter(uid => !pendingIds.has(uid));
  const action = pending.length ? (recorded.length ? '删除 / 丢弃' : '丢弃') : '删除';
  const only = uids.length === 1
    ? (sidebarSessions().find(x => x.uid === uids[0])?.title || '') : '';
  const running = recorded.filter(uid => live.live.has(uid)).length;
  // OpenCode keeps sessions in its own database: they are deleted there,
  // with their child sessions, and never reach the recycle bin.
  const opencode = recorded.filter(uid => sidebarSessions().find(x => x.uid === uid)?.source === 'opencode');
  // Unpersisted launches discard immediately. Recorded sessions still confirm
  // because they move into the recycle bin.
  if (recorded.length && !await appConfirm((uids.length === 1
      ? `${action}会话「${only}」?\n\n` : `${action}选中的 ${uids.length} 个会话?\n\n`)
    + (pending.length ? `${pending.length} 个新建会话将停止并丢弃，未发送的草稿也会清除；若已生成会话记录，记录会保留。\n` : '')
    + (opencode.length < recorded.length ? trashLocationNote() : '')
    + (opencode.length ? (opencode.length === recorded.length ? '' : '\n')
      + (opencode.length === 1 && uids.length === 1 ? '' : `其中 ${opencode.length} 个 `) + opencodeDeleteNote() : '')
    + (running ? `\n其中 ${running} 个还在运行，会被跳过，需要先停止。` : '')))
    return null;
  state.deleteBusy = true;
  if (button) button.disabled = true;
  const watched = recorded.includes(selection.sel) ? selection.sel : null;
  if (watched) closeWatch();   // 文件即将移走，先停掉这条 SSE
  const d = { deleted: [], errors: [] };
  try {
    for (const info of pending) {
      try {
        await discardPendingSession(info);
        d.deleted.push({ uid: info.uid });
      } catch (e) {
        d.errors.push({ uid: info.uid, title: info.title, error: e.message });
      }
    }
    if (recorded.length) {
      try {
        const postDelete = async (uids, force = false) => {
          const r = await fetch(appUrl('api/sessions/delete'), {
            method: 'POST', headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(force ? { uids, force: true } : { uids }),
          });
          const result = await r.json();
          if (!r.ok || result.error) throw new Error(result.error || `HTTP ${r.status}`);
          return result;
        };
        const result = await postDelete(recorded);
        d.deleted.push(...(result.deleted || []));
        let errors = result.errors || [];
        // Rust 回收站：运行状态未知的会话先被跳过，用户确认后才带 force 重试。
        const unknown = trashCapable() ? (result.skipped || []).filter(x => x.needs_force) : [];
        if (unknown.length && await confirmForceDelete(unknown.length, unknown[0].run_state?.detail)) {
          const forced = await postDelete(unknown.map(x => x.uid), true);
          d.deleted.push(...(forced.deleted || []));
          const retried = new Set(unknown.map(x => x.uid));
          errors = errors.filter(x => !retried.has(x.uid)).concat(forced.errors || []);
        }
        d.errors.push(...errors);
      } catch (e) {
        d.errors.push(...recorded.map(uid => ({ uid, error: e.message })));
      }
    }
    if (pending.length) await loadTermList();
  } finally {
    state.deleteBusy = false;
    if (button) button.disabled = false;
  }
  const gone = new Set((d.deleted || []).map(x => x.uid));
  const failed = d.errors || [];
  if (watched && selection.sel === watched && !gone.has(watched)) watchSession(watched, selection.agent);
  if (gone.size) {
    forgetDeletedReceipts(catalog.sessions.filter(x => gone.has(x.uid)));
    catalog.sessions = catalog.sessions.filter(x => !gone.has(x.uid));
    if (search.results) search.results = search.results.filter(x => !gone.has(x.uid));
    if (gone.has(selection.sel)) {
      selection.sel = null;
      store.set('sel', null);
      const trashed = !opencode.includes(watched || '');
      mountDeleted({opencode:!trashed});
      ensureConsolePlaceholder();
      auditDetailRendered('trashed');
      showMobileList();
    }
  }
  renderChips();
  renderSide();
  if (failed.length === 1 && uids.length === 1) {
    await appAlert(`${action}失败: ` + failed[0].error);
  } else if (failed.length) {
    const lines = failed.slice(0, 5).map(x => `· ${x.title || x.uid}: ${x.error}`);
    await appAlert(`已${action} ${gone.size} 个，${failed.length} 个操作失败:\n\n`
      + lines.join('\n') + (failed.length > 5 ? '\n…' : ''));
  }
  return { gone, failed };
}

async function deletePickedSessions() {
  const result = await deleteSessions([...state.picked], $('#side-pick-delete'));
  if (!result) return;
  // 删不掉的（多半还在运行）留在选择里，用户停掉会话后可以直接再点删除。
  state.picked.clear();
  result.failed.forEach(x => state.picked.add(x.uid));
  if (result.failed.length) { renderSide(); renderPickBar(); } else setPicking(false);
}

async function stopPickedSessions() {
  if (state.stopBusy || state.deleteBusy) return;
  const targets = pickedStopTargets();
  if (!targets.length) return;
  // 全部停在输入框（空闲）时直接停；有在轮转、等回答或状态未知的才确认。
  if (!targets.every(s => !s.pending && sessionTurn(s.uid) === 'idle')
      && !await appConfirm(`停止所选的 ${targets.length} 个运行中会话?\n\n会话记录和草稿会保留，已结束的会话会跳过。`)) return;
  state.stopBusy = true;
  const progress = state.stopProgress = {total: targets.length, stopped: 0, settled: 0,
    failed: 0, uncertain: 0, details: [], refreshError: false};
  showSessionStopNotice('');
  $('#side-stop-details').open = false;
  renderPickBar();
  let next = 0;
  const stopNext = async () => {
    while (next < targets.length) {
      const target = targets[next++];
      try {
        if (target.pending) {
          const result = await post('api/term/kill', { record_id: target.record_id,
            instance_id: target.instance_id, ...(HUB_MODE ? { _node: target.node_id } : {}) });
          if (result.error) throw new Error(result.error);
          const current = terminal.state.pending.find(row => row.record_id === target.record_id && row.node_id === target.node_id);
          if (current) Object.assign(current, result);
          if (!['exited', 'failed'].includes(result.state)) {
            progress.uncertain++;
            progress.details.push(`「${target.title}」停止请求已发送，尚未确认退出`);
            continue;
          }
        } else {
          const result = await requestSessionStop(target);
          if (result.stage === 'uncertain') {
            progress.uncertain++;
            progress.details.push(`「${target.title}」${STOP_STAGE_TEXT.uncertain}`);
            continue;
          }
        }
        progress.stopped++;
      } catch (error) {
        progress.failed++;
        progress.details.push(`「${target.title}」停止失败：${error.message || error}`);
      } finally {
        progress.settled++;
        renderPickBar();
      }
    }
  };
  try {
    await Promise.all(Array.from({length: Math.min(sessionStopConcurrency(), targets.length)}, stopNext));
    await refreshLive(true);
    if (typeof loadTermList === 'function') await loadTermList();
    paintLive();
  } catch (error) {
    progress.refreshError = true;
    progress.details.push(`停止请求已处理，刷新状态失败：${error.message || error}`);
  } finally {
    state.stopBusy = false;
    renderPickBar();
  }
}

/** 选中的会话里能改附属关系的：真实会话行，不是待定启动或分叉父行。 */
function pickedNestable() {
  if (!capabilities.allows('metadata')) return [];
  const rows = new Map(sidebarSessions().map(session => [session.uid, session]));
  return [...state.picked].filter(uid => {
    const row = rows.get(uid);
    return !!row && !row.pending && !row.fork_parent;
  });
}

/* ---------- 会话行的右键 / 长按菜单 ---------- */
// 删除入口不再常驻占位：右键（手机长按）某条会话，才给出删除和进入多选。
  return {state, gesture, sessionPickable, sessionStopConcurrency, pickedStopTargets, syncPickedSessions, setPicking, toggleSessionPick, toggleGroupPick, pickDragRows, pickDragTo, pickDragAt, pickDragScroll, endPickDrag, sidebarGestureMousedown1, sidebarGestureSelectstart1, sidebarGestureClick1, pickAllVisible, renderPickBar, deleteSessions, deletePickedSessions, stopPickedSessions, pickedNestable};
}
