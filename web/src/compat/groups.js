'use strict';

// Nodes own groups and assignments; the Hub distributes catalog changes.
globalThis.SessionDockGroups = (() => {
  let catalog = [], available = false, refreshTask = null, busy = false;
  let menuUids = [], menuAnchor = null, editing = false;
  const menu = $('#session-group-menu'), status = $('#session-group-status');
  const row = uid => indexedSessions().byUid.get(uid);
  const enabled = SessionDockCapabilities.allows('metadata');
  $('#view [data-v="group"]').hidden = !enabled;
  if (!enabled && S.view === 'group') { S.view = 'tree'; renderView(); renderSide(); }
  function message(text) { status.textContent = text; }
  function absorb(data) {
    const next = [...new Set(data.groups || [])].sort((a, b) => a.localeCompare(b));
    const changed = !available || JSON.stringify(next) !== JSON.stringify(catalog);
    catalog = next; available = true;
    paintPickBar(!!S.nestAttach);
    if (changed) renderSide();
    if (!menu.hidden) paintMenu();
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
    return S.view !== 'group' || !!session.group && (!available || catalog.includes(session.group));
  }
  function paintPickBar() {renderPickBar();}
  function setBusy(value) {busy = value; renderSide(); if (!menu.hidden) paintMenu();}
  async function create(input) {
    if (busy) return;
    const name = input.value.trim(); if (!name) { input.focus(); return; }
    setBusy(true); message('');
    try {
      const data = await request('api/groups', {create_groups: [name]});
      editing = false; absorb(data); renderSide();
      message(data.sync_errors?.length ? '已创建；离线节点恢复连接后同步。' : '');
    } catch (error) { message(error.message); }
    finally { setBusy(false); }
  }
  async function remove(name) {
    if (busy) return;
    setBusy(true); message('');
    try {
      const data = await request('api/groups', {delete_groups: [name]});
      absorb(data); await loadSessions(true); renderSide();
      message(data.sync_errors?.length ? '已删除；离线节点恢复连接后同步。' : '');
    } catch (error) { message(error.message); }
    finally { setBusy(false); }
  }
  function edit(on) {editing = on; renderSide();}
  function applyAssignment(uid, data) {
    for (const session of [...S.sessions, ...(S.results || [])]) if (session.uid === uid) session.group = data.group || null;
    for (const entry of cache.values()) if (entry.meta.uid === uid && !entry.meta.agent_id) entry.meta.group = data.group || null;
  }
  async function assign(group) {
    if (busy) return;
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
    SessionDockSidebar.updateGroupMenu(['', ...catalog].map(name => ({name,
      checked: menuUids.length > 0 && menuUids.every(uid => (catalog.includes(row(uid)?.group) ? row(uid).group : '') === name)})),
      busy, name => void assign(name), groupMenuKeydown);
    if (active !== undefined) [...menu.children].find(button => button.dataset.groupName === active)?.focus();
  }
  function showMenu(uids, anchor, focus = true) {
    if (!available || busy || anchor.getAttribute('aria-disabled') === 'true') return;
    menuUids = [...new Set(uids)].filter(uid => row(uid) && !row(uid).pending);
    if (!menuUids.length) return;
    menuAnchor?.setAttribute('aria-expanded', 'false'); menuAnchor = anchor;
    anchor.setAttribute('aria-expanded', 'true'); paintMenu(); menu.hidden = false;
    const box = anchor.getBoundingClientRect(), width = menu.offsetWidth, height = menu.offsetHeight;
    const right = box.right + 5, left = box.left - width - 5;
    menu.style.left = `${Math.max(8, Math.min(right + width <= innerWidth - 8 ? right : left, innerWidth - width - 8))}px`;
    menu.style.top = `${Math.max(8, Math.min(box.top, innerHeight - height - 8))}px`;
    if (focus) (menu.querySelector('[aria-checked="true"]') || menu.firstElementChild)?.focus();
  }
  function closeMenu() { menu.hidden = true; menuAnchor?.setAttribute('aria-expanded', 'false'); menuAnchor = null; menuUids = []; }
  function escapeMenu() { if (menu.hidden) return false; const anchor = menuAnchor; closeMenu(); anchor?.focus(); return true; }
  function groupMenuKeydown(event) {
    if (event.key === 'ArrowLeft' || event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); escapeMenu(); }
    else if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
      event.preventDefault(); const buttons = [...menu.querySelectorAll('button')], index = buttons.indexOf(document.activeElement);
      buttons[event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1 : (index + (event.key === 'ArrowDown' ? 1 : buttons.length - 1)) % buttons.length]?.focus();
    }
  }
  const trigger = $('#item-menu [data-act="group"]');
  trigger.onpointerenter = event => { if (event.pointerType === 'mouse') showMenu([menuUid], trigger, false); };
  trigger.onkeydown = event => { if (event.key === 'ArrowRight') { event.preventDefault(); showMenu([menuUid], trigger); } };
  $('#item-menu').addEventListener('pointerover', event => { if (event.target.closest('button[data-act]')?.dataset.act !== 'group') closeMenu(); });

  document.addEventListener('pointerdown', event => { if (!event.target.closest('#session-group-menu, #item-menu, #side-pick-group')) closeMenu(); }, true);
  addEventListener('resize', closeMenu);
  void refresh();
  setInterval(() => { if (!document.hidden && !busy && !editing) void refresh(); }, 10000);
  return {matches, paintPickBar, create, remove, edit, showMenu, closeMenu, escapeMenu,
    get busy() {return busy;}, get editing() {return editing;},
    contains: name => catalog.includes(name), get names() { return catalog; }, get available() { return available; }};
})();
