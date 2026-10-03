import {nextTick} from 'vue'
import {createItemMenuPresentation} from '../sidebar/item-menu'
const $ = selector => document.querySelector(selector);
import * as SessionUi from '../../migration/session-ui'
import {copyFileText} from '../overlays/files'

export function createSidebarGestures({core,sessionUi,terminal,sidebarView,status,bulk,build,metadata,capabilities}) {
const LONG_PRESS_MS = 480;

const LONG_PRESS_SLOP = 12;

const itemMenu = createItemMenuPresentation({action:itemMenuAction,
  groupPointerenter(event){if(event.pointerType==='mouse') core.groups?.showMenu([itemMenu.state.uid],event.currentTarget,false)},
  groupKeydown(event){if(event.key==='ArrowRight'){event.preventDefault();core.groups?.showMenu([itemMenu.state.uid],event.currentTarget)}},
  pointerover(event){if(event.target.closest('button[data-act]')?.dataset.act!=='group')core.groups?.closeMenu()},
});

let longPress = { timer: 0, x: 0, y: 0 };

let suppressItemClick = false;

async function openItemMenu(uid, x, y) {
  core.groups?.closeMenu();
  const list = core.index.sidebarSessions();
  const row = list.find(session => session.uid === uid);
  const parent = !!row?.fork_parent;
  const running = sessionUi().sessionStoppable(uid);
  const unusedLaunch = !row?.pending && sessionUi().unusedNewAssignedLaunch(row);
  // 运行中的 SSH 会话先停止，结束后才删除；未使用的原生启动仍可直接丢弃。
  const shellRunning = typeof terminal().pendingShellRunning === 'function' && terminal().pendingShellRunning(row);
  const nested = !!(row?.nest_parent?.source && row.nest_parent.sid);
  const nestable = capabilities.allows('metadata') && !!row && !row.pending && !parent;
  // Keep every action in its fixed position, with the same unavailable hints as
  // other controls. The shared capture handler blocks mouse, touch and keyboard clicks.
  const unavailable = {
    'copy-identity': !row?.sid ? '此会话尚无原生会话标识。' : '',
    stop: parent || (row?.pending ? !shellRunning : !running || !!unusedLaunch)
      ? '此会话当前没有可停止的进程。' : '',
    hide: !parent ? '仅分叉父会话可隐藏。' : '',
    detach: !nestable || !nested ? '此会话当前没有附属关系。' : '',
    attach: !nestable ? '此会话当前不能设置附属关系。' : '',
    delete: parent ? '分叉父会话可隐藏，不能直接删除。'
      : ((!row?.pending && running && !unusedLaunch) || shellRunning)
        ? '请先停止会话再删除。' : '',
    group: !capabilities.allows('metadata') || !row || row.pending || !core.groups?.available
      ? '此会话当前不能保存分组。' : '',
    pick: parent ? '分叉父会话不能加入多选。' : '',
  };
  unavailable.clone = sessionUi().itemTransferUnavailableReason(uid);
  itemMenu.open(uid, ((row?.pending && row?.source !== 'shell') || unusedLaunch) ? '丢弃会话' : '删除会话', unavailable);
  await nextTick();
  const menu = $('#item-menu');
  const box = menu.getBoundingClientRect();
  itemMenu.position(Math.max(8, Math.min(x, innerWidth - box.width - 8)), Math.max(8, Math.min(y, innerHeight - box.height - 8)));
  menu.querySelector('button:not([aria-disabled="true"])')?.focus({ preventScroll: true });
}

function closeItemMenu() {
  core.groups?.closeMenu();
  itemMenu.close();
}

function cancelLongPress() {
  clearTimeout(longPress.timer);
  longPress.timer = 0;
}

const menuTarget = e => e.target.closest('#side .item');

let sidebarRenderDeferred = false;

let sidebarRenderFlushTimer = 0;

let sidebarTextPointer = null;

function sidebarTextSelectionActive() {
  const selection = getSelection();
  const side = $('#side');
  return !!selection && !selection.isCollapsed && !!selection.toString()
    && !!side && side.contains(selection.anchorNode) && side.contains(selection.focusNode);
}

const sidebarTextSelectionProtected = () => !!sidebarTextPointer || sidebarTextSelectionActive();

function scheduleDeferredSidebarRender() {
  if (!sidebarRenderDeferred || sidebarTextSelectionProtected() || sidebarRenderFlushTimer) return;
  sidebarRenderFlushTimer = setTimeout(flushDeferredSidebarRender, 0);
}

function flushDeferredSidebarRender() {
  sidebarRenderFlushTimer = 0;
  if (!sidebarRenderDeferred || sidebarTextSelectionProtected()) return;
  const side = $('#side');
  const top = side?.scrollTop || 0;
  sidebarRenderDeferred = false;
  sidebarView().renderSide();
  if (side) side.scrollTop = top;
  status().paintLive();
}

function sidebarGesturePointerdown1(event) {
  if (event.pointerType !== 'mouse' || event.button !== 0 || core.state.sidebar.picking
      || !event.target.closest('.t, .m, .cwd, .snip, .gname')) return;
  sidebarTextPointer = {id: event.pointerId, x: event.clientX, y: event.clientY, moved: false};
}

function sidebarGestureClick2(event) {
  const dragged = !!sidebarTextPointer?.moved;
  sidebarTextPointer = null;
  if (!dragged && !sidebarTextSelectionActive()) {
    scheduleDeferredSidebarRender();
    return;
  }
  event.stopPropagation();
  event.preventDefault();
}

function sidebarGestureContextmenu1(e) {
  const row = menuTarget(e);
  if (!row || core.state.sidebar.picking || core.state.sidebar.nestAttach) return;      // 选择/附属点选里点选就够了，不再叠一层菜单
  e.preventDefault();
  openItemMenu(row.dataset.uid, e.clientX, e.clientY);
}

function sidebarGesturePointerdown2(e) {
  if (e.pointerType === 'mouse') return;             // 鼠标走 contextmenu
  const row = menuTarget(e);
  if (!row || core.state.sidebar.picking || core.state.sidebar.nestAttach) return;
  cancelLongPress(); // A second finger must not leave the first hold timer alive.
  longPress = { timer: 0, x: e.clientX, y: e.clientY };
  longPress.timer = setTimeout(() => {
    longPress.timer = 0;
    suppressItemClick = true;                        // 长按不该顺手打开会话
    navigator.vibrate?.(12);
    openItemMenu(row.dataset.uid, longPress.x, longPress.y);
  }, LONG_PRESS_MS);
}

function sidebarGesturePointermove1(e) {
  if (!longPress.timer) return;
  if (Math.abs(e.clientX - longPress.x) > LONG_PRESS_SLOP
      || Math.abs(e.clientY - longPress.y) > LONG_PRESS_SLOP) cancelLongPress();
}

function sidebarGestureClick3(e) {
  if (!suppressItemClick) return;
  suppressItemClick = false;
  e.stopPropagation();
  e.preventDefault();
}

function sidebarVueGestures() {return {mousedown: event => {bulk().sidebarGestureMousedown1(event);},
pointerdown: event => {sidebarGesturePointerdown1(event); sidebarGesturePointerdown2(event);},
pointermove: event => {sidebarGesturePointermove1(event);},
pointerup: event => {cancelLongPress();},
pointercancel: event => {cancelLongPress();},
pointerleave: event => {if (!$('#side').contains(event.relatedTarget)) cancelLongPress();},
contextmenu: event => {sidebarGestureContextmenu1(event);},
clickCapture: event => {bulk().sidebarGestureClick1(event); sidebarGestureClick2(event); sidebarGestureClick3(event);},
selectstart: event => {bulk().sidebarGestureSelectstart1(event);}};
}
async function itemMenuAction(action, button) {
  const uid = itemMenu.state.uid;
  if (action === 'group') { core.groups?.showMenu([uid], button); return; }
  closeItemMenu();
  if (!uid) return;
  if (action === 'copy-identity') {
    const row = core.index.sidebarSessions().find(session => session.uid === uid);
    if (!row?.sid) return;
    const machine = row.node_name || core.state.nodes.list.find(node => node.id === row.node_id)?.name
      || (!core.environment.HUB_MODE ? build().state.hostname : '') || '(未知)';
    const text = `机器：${machine}\n目录：${row.cwd || '(未知)'}\nagent：${row.source}\nUUID：${row.sid}`;
    try {
      await copyFileText(text);
      sessionUi().showSessionStopNotice('会话标识已复制。');
    } catch (error) {
      await SessionUi.appAlert(`复制会话标识失败：${error.message || error}`);
    }
    return;
  }
  if (action === 'clone') { await sessionUi().cloneSessionGroup(uid); return; }
  if (action === 'hide') {
    await metadata().setForkParentVisibility([uid], false);
    return;
  }
  if (action === 'detach') {
    await sidebarView().setSessionNest(uid, {parent_uid: null});
    return;
  }
  if (action === 'attach') {
    sidebarView().setNestAttach(uid);
    return;
  }
  if (action === 'pick') {
    bulk().state.picked.add(uid);       // 从哪条进入多选，就先勾上哪条
    bulk().setPicking(true);
    return;
  }
  if (action === 'stop') {
    const row = core.state.catalog.sessions.find(x => x.uid === uid) || core.index.sidebarSessions().find(x => x.uid === uid);
    if (row?.pending) { if (typeof terminal().stopPendingSession === 'function') await terminal().stopPendingSession(row); return; }
    if (row) await sessionUi().stopSession(row);
    return;
  }
  const row = core.index.sidebarSessions().find(session => session.uid === uid);
  const launch = sessionUi().unusedNewAssignedLaunch(row);
  if (launch && typeof terminal().deletePendingSession === 'function') {
    await terminal().deletePendingSession(launch);
    return;
  }
  await bulk().deleteSessions([uid]);
}

function start(){
document.addEventListener('selectionchange', () => {
  scheduleDeferredSidebarRender();
});
document.addEventListener('pointermove', event => {
  if (longPress.timer && $('#side').contains(event.target)) sidebarGesturePointermove1(event);
  if (!sidebarTextPointer || sidebarTextPointer.id !== event.pointerId) return;
  if (Math.abs(event.clientX - sidebarTextPointer.x) > 3
      || Math.abs(event.clientY - sidebarTextPointer.y) > 3) sidebarTextPointer.moved = true;
});
document.addEventListener('pointerup', event => {
  if (longPress.timer && $('#side').contains(event.target)) cancelLongPress();
  if (!sidebarTextPointer || sidebarTextPointer.id !== event.pointerId) return;
  const pointer = sidebarTextPointer;
  setTimeout(() => {
    if (sidebarTextPointer !== pointer) return;
    sidebarTextPointer = null;
    scheduleDeferredSidebarRender();
  }, 0);
});
document.addEventListener('pointercancel', event => {
  if (longPress.timer && $('#side').contains(event.target)) cancelLongPress();
  if (!sidebarTextPointer || sidebarTextPointer.id !== event.pointerId) return;
  sidebarTextPointer = null;
  scheduleDeferredSidebarRender();
});
$('#side').addEventListener('scroll', () => {cancelLongPress(); closeItemMenu();});
document.addEventListener('pointerdown', e => {
  if (!itemMenu.state.hidden && !e.target.closest('#item-menu, #session-group-menu')) closeItemMenu();
}, true);
addEventListener('resize', closeItemMenu);
}
return {resetItemClick(){suppressItemClick=false},LONG_PRESS_MS,LONG_PRESS_SLOP,get menuUid(){return itemMenu.state.uid},get menuState(){return itemMenu.state},setGroupExpanded:itemMenu.setGroupExpanded,setCloneReason(reason){itemMenu.state.reasons.clone=reason},itemMenuAction,get longPress(){return longPress},get suppressItemClick(){return suppressItemClick},openItemMenu,closeItemMenu,cancelLongPress,menuTarget,get sidebarRenderDeferred(){return sidebarRenderDeferred},set sidebarRenderDeferred(value){sidebarRenderDeferred=value},get sidebarRenderFlushTimer(){return sidebarRenderFlushTimer},get sidebarTextPointer(){return sidebarTextPointer},sidebarTextSelectionActive,sidebarTextSelectionProtected,scheduleDeferredSidebarRender,flushDeferredSidebarRender,sidebarGesturePointerdown1,sidebarGestureClick2,sidebarGestureContextmenu1,sidebarGesturePointerdown2,sidebarGesturePointermove1,sidebarGestureClick3,sidebarVueGestures,start};
}
