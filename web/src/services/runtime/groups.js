import {mountGroupMenu} from './group-menu-view'
import {nextTick} from 'vue'
import {useGroupsStore} from '../../stores/runtime/groups'

export function createGroupsService(pinia, environment, sessions, sidebar, itemMenu) {
  const {capabilities, fetch, appUrl} = environment;
  const {catalog, sidebar: sidebarState, search, cache, indexedSessions, loadSessions} = sessions;
  const {renderView, renderSide, renderPickBar} = sidebar;
  const closeItemMenu = itemMenu.close;
  const $ = selector => document.querySelector(selector);
  const state = useGroupsStore(pinia);
  let refreshTask = null;
  let menuUids = [], menuAnchor = null;
  let menu = $('#session-group-menu');
  const row = uid => indexedSessions().byUid.get(uid);
  const enabled = capabilities.allows('metadata');
  function message(text) { state.message = text; }
  function absorb(data) {
    const next = [...new Set(data.groups || [])].sort((a, b) => a.localeCompare(b));
    const changed = !state.available || JSON.stringify(next) !== JSON.stringify(state.catalog);
    state.catalog = next; state.available = true;
    paintPickBar(!!sidebarState.nestAttach);
    if (changed) renderSide();
    if (!state.menuHidden) paintMenu();
  }
  async function request(path, body) {
    const response = await fetch(appUrl(path), body === undefined ? {} : {
      method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify(body),
    });
    const data = await response.json();
    if (!response.ok) throw new Error(data.error || (data.sync_errors?.length ? '没有节点确认保存，请稍后重试' : `HTTP ${response.status}`));
    return data;
  }
  async function refresh() {
    if (!enabled) return;
    if (refreshTask) return refreshTask;
    refreshTask = request('api/groups').then(absorb).catch(error => { console.warn('分组读取失败', error); }).finally(() => { refreshTask = null; });
    return refreshTask;
  }
  function matches(session) {
    return sidebarState.view !== 'group' || !!session.group && (!state.available || state.catalog.includes(session.group));
  }
  function paintPickBar() {renderPickBar();}
  function setBusy(value) {state.busy = value; renderSide(); if (!state.menuHidden) paintMenu();}
  async function create(input) {
    if (state.busy) return;
    const name = input.value.trim(); if (!name) { input.focus(); return; }
    setBusy(true); message('');
    try {
      const data = await request('api/groups', {create_groups: [name]});
      state.editing = false; absorb(data); renderSide();
      message(data.sync_errors?.length ? '已创建；离线节点恢复连接后同步。' : '');
    } catch (error) { message(error.message); }
    finally { setBusy(false); }
  }
  async function remove(name) {
    if (state.busy) return;
    setBusy(true); message('');
    try {
      const data = await request('api/groups', {delete_groups: [name]});
      absorb(data); await loadSessions(true); renderSide();
      message(data.sync_errors?.length ? '已删除；离线节点恢复连接后同步。' : '');
    } catch (error) { message(error.message); }
    finally { setBusy(false); }
  }
  function edit(on) {state.editing = on; renderSide();}
  function applyAssignment(uid, data) {
    for (const session of [...catalog.sessions, ...(search.results || [])]) if (session.uid === uid) session.group = data.group || null;
    for (const entry of cache.values()) if (entry.meta.uid === uid && !entry.meta.agent_id) entry.meta.group = data.group || null;
  }
  async function assign(group) {
    if (state.busy) return;
    const selected = [...menuUids]; closeItemMenu(); closeMenu(); setBusy(true); message('');
    const failed = [];
    try {
      for (const uid of selected) {
        try { applyAssignment(uid, await request('api/session/group', {uid, set_group: true, group: group || null})); }
        catch (error) { failed.push(`${row(uid)?.title || uid}：${error.message}`); }
      }
      await loadSessions(true); renderSide();
      if (failed.length) message(`保存失败：${failed.join('；')}`);
    } finally { setBusy(false); }
  }
  function paintMenu() {
    const active = document.activeElement?.dataset.groupName;
    state.menuItems=['', ...state.catalog].map(name => ({name,
      checked: menuUids.length > 0 && menuUids.every(uid => (state.catalog.includes(row(uid)?.group) ? row(uid).group : '') === name)}));

    if (active !== undefined) [...menu.children].find(button => button.dataset.groupName === active)?.focus();
  }
  async function showMenu(uids, anchor, focus = true) {
    if (!state.available || state.busy || anchor.getAttribute('aria-disabled') === 'true') return;
    menuUids = [...new Set(uids)].filter(uid => row(uid) && !row(uid).pending);
    if (!menuUids.length) return;
    if(menuAnchor?.closest('#item-menu'))itemMenu.setGroupExpanded(false);else menuAnchor?.setAttribute('aria-expanded','false');menuAnchor=anchor;
    if(anchor.closest('#item-menu'))itemMenu.setGroupExpanded(true);else anchor.setAttribute('aria-expanded','true'); paintMenu(); state.menuHidden = false;await nextTick();
    const box = anchor.getBoundingClientRect(), width = menu.offsetWidth, height = menu.offsetHeight;
    const right = box.right + 5, left = box.left - width - 5;
    state.menuLeft = `${Math.max(8, Math.min(right + width <= innerWidth - 8 ? right : left, innerWidth - width - 8))}px`;
    state.menuTop = `${Math.max(8, Math.min(box.top, innerHeight - height - 8))}px`;
    if (focus) (menu.querySelector('[aria-checked="true"]') || menu.firstElementChild)?.focus();
  }
  function closeMenu() { state.menuHidden = true; if(menuAnchor?.closest('#item-menu'))itemMenu.setGroupExpanded(false);else menuAnchor?.setAttribute('aria-expanded', 'false'); menuAnchor = null; menuUids = []; }
  function escapeMenu() { if (state.menuHidden) return false; const anchor = menuAnchor; closeMenu(); anchor?.focus(); return true; }
  function groupMenuKeydown(event) {
    if (event.key === 'ArrowLeft' || event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); escapeMenu(); }
    else if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
      event.preventDefault(); const buttons = [...menu.querySelectorAll('button')], index = buttons.indexOf(document.activeElement);
      buttons[event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1 : (index + (event.key === 'ArrowDown' ? 1 : buttons.length - 1)) % buttons.length]?.focus();
    }
  }
  function start() {
  menu=mountGroupMenu(name=>void assign(name),groupMenuKeydown);
  state.enabled=enabled;
  if (!enabled && sidebarState.view === 'group') { sidebarState.view = 'tree'; renderView(); renderSide(); }
  document.addEventListener('pointerdown', event => { if (!event.target.closest('#session-group-menu, #item-menu, #side-pick-group')) closeMenu(); }, true);
  addEventListener('resize', closeMenu);
  void refresh();
  const timer = setInterval(() => { if (!document.hidden && !state.busy && !state.editing) void refresh(); }, 10000);
  return () => clearInterval(timer);
  }
  return {start, matches, paintPickBar, create, remove, edit, showMenu, closeMenu, escapeMenu,
    get busy() {return state.busy;}, get editing() {return state.editing;},
    contains: name => state.catalog.includes(name), get names() {return state.catalog;}, get available() {return state.available;}};
}
