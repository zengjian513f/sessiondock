'use strict';

// Nodes own groups and assignments; the Hub distributes catalog changes.
globalThis.SessionDockGroups = (() => {
  let catalog = [], available = false, refreshTask = null, busy = false;
  let menuUids = [], menuAnchor = null, editing = false, menuEditing = false;
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
  function paintRow(node, session) {
    const body = node.querySelector('.body'); if (!body) return;
    let badge = body.querySelector('.session-row-group');
    if (!badge) { badge = document.createElement('div'); badge.className = 'session-row-group'; body.append(badge); }
    badge.textContent = session.group && (!available || catalog.includes(session.group)) ? `分组：${session.group}` : '';
  }
  function paintPickBar(attaching) {
    const button = $('#side-pick-group');
    button.hidden = attaching || !available;
    button.disabled = busy || ![...pickedSessions].some(uid => row(uid) && !row(uid).pending);
  }
  function setBusy(value) {
    busy = value; paintPickBar(!!S.nestAttach);
    for (const button of document.querySelectorAll('.session-group-delete, #session-group-create-row button, #session-group-menu button')) button.disabled = value;
    const input = $('#session-group-name'); if (input) input.disabled = value;
    const menuInput = $('#session-group-menu-name'); if (menuInput) menuInput.disabled = value;
  }
  async function create(input, uids = null) {
    if (busy) return;
    const name = input.value.trim(); if (!name) { input.focus(); return; }
    setBusy(true); message('');
    let created = false, syncMessage = '';
    try {
      const data = await request('api/groups', {create_groups: [name]});
      created = true;
      if (uids) menuEditing = false; else editing = false;
      absorb(data); renderSide();
      syncMessage = data.sync_errors?.length ? '已创建；离线节点恢复连接后同步。' : '';
      message(syncMessage);
    } catch (error) { message(error.message); }
    finally { setBusy(false); }
    if (created && uids) {
      await assign(name, uids);
      if (syncMessage && !status.textContent) message(syncMessage);
    } else if (!created && input.isConnected) input.focus();
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
  function paintHeading(head, name) {
    let button = head.querySelector('.session-group-delete');
    if (!button) { button = document.createElement('button'); button.type = 'button'; button.className = 'session-group-delete'; head.append(button); }
    button.textContent = '×'; button.setAttribute('aria-label', `删除分组 ${name}`); button.title = `删除分组 ${name}`;
    button.disabled = busy;
    button.onclick = event => { event.stopPropagation(); void remove(name); };
  }
  function paintSidebar(side) {
    if (S.view !== 'group' || !available) { $('#session-group-create-row')?.remove(); return; }
    let host = $('#session-group-create-row');
    if (!host) { host = document.createElement('div'); host.id = 'session-group-create-row'; }
    if (!editing && !host.querySelector('#session-group-add')) {
      const button = document.createElement('button'); button.id = 'session-group-add'; button.type = 'button';
      button.textContent = '＋ 新建分组'; button.disabled = busy;
      button.onclick = () => { editing = true; paintSidebar(side); $('#session-group-name').focus(); };
      host.replaceChildren(button);
    } else if (editing && !host.querySelector('input')) {
      const form = document.createElement('form'), input = document.createElement('input'), save = document.createElement('button');
      input.id = 'session-group-name'; input.placeholder = '分组名称'; input.setAttribute('aria-label', '新分组名称'); input.autocomplete = 'off';
      save.type = 'submit'; save.textContent = '创建'; save.disabled = busy;
      form.append(input, save); form.onsubmit = event => { event.preventDefault(); void create(input); };
      input.onkeydown = event => { if (event.key === 'Escape') { event.stopPropagation(); editing = false; paintSidebar(side); $('#session-group-add').focus(); } };
      host.replaceChildren(form);
    }
    if (host.parentElement !== side || side.lastElementChild !== host) side.append(host);
  }
  function applyAssignment(uid, data) {
    for (const session of [...S.sessions, ...(S.results || [])]) if (session.uid === uid) session.group = data.group || null;
    for (const entry of cache.values()) if (entry.meta.uid === uid && !entry.meta.agent_id) entry.meta.group = data.group || null;
  }
  async function assign(group, uids = menuUids) {
    if (busy) return;
    const selected = [...uids]; closeItemMenu(); closeMenu(); setBusy(true); message('');
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
    const createFocused = document.activeElement === $('#session-group-menu-add');
    const form = menu.querySelector('form'), inputFocused = form?.contains(document.activeElement);
    menu.replaceChildren();
    for (const name of ['', ...catalog]) {
      const button = document.createElement('button'); button.type = 'button'; button.dataset.groupName = name;
      button.setAttribute('role', 'menuitemradio');
      const checked = menuUids.length > 0 && menuUids.every(uid => (catalog.includes(row(uid)?.group) ? row(uid).group : '') === name);
      button.setAttribute('aria-checked', String(checked)); button.disabled = busy;
      const mark = document.createElement('span'); mark.className = 'group-menu-check'; mark.setAttribute('aria-hidden', 'true'); mark.textContent = checked ? '✓' : '';
      button.append(mark, document.createTextNode(name || '未分组')); button.onclick = () => void assign(name);
      menu.append(button);
    }
    const separator = document.createElement('div'); separator.setAttribute('role', 'separator');
    menu.append(separator);
    if (menuEditing) {
      if (form) menu.append(form);
      else {
        const form = document.createElement('form'), input = document.createElement('input');
        input.id = 'session-group-menu-name'; input.placeholder = '分组名称'; input.setAttribute('aria-label', '新分组名称'); input.autocomplete = 'off'; input.disabled = busy;
        form.append(input);
        form.onsubmit = event => { event.preventDefault(); void create(input, [...menuUids]); };
        input.onkeydown = event => {
          event.stopPropagation();
          if (event.key === 'Enter' && event.isComposing) event.preventDefault();
          if (event.key === 'Escape') {
            event.preventDefault(); menuEditing = false; paintMenu(); $('#session-group-menu-add').focus();
          }
        };
        menu.append(form);
      }
    } else {
      const button = document.createElement('button'); button.id = 'session-group-menu-add'; button.type = 'button'; button.setAttribute('role', 'menuitem'); button.disabled = busy;
      const mark = document.createElement('span'); mark.className = 'group-menu-check'; mark.setAttribute('aria-hidden', 'true'); mark.textContent = '＋';
      button.append(mark, document.createTextNode('新建分组'));
      button.onclick = () => { menuEditing = true; paintMenu(); positionMenu(); $('#session-group-menu-name').focus(); };
      menu.append(button);
    }
    if (active !== undefined) [...menu.children].find(button => button.dataset.groupName === active)?.focus();
    else if (createFocused) $('#session-group-menu-add')?.focus();
    else if (inputFocused) $('#session-group-menu-name')?.focus();
  }
  function positionMenu() {
    if (!menuAnchor) return;
    const box = menuAnchor.getBoundingClientRect(), width = menu.offsetWidth, height = menu.offsetHeight;
    const right = box.right + 5, left = box.left - width - 5;
    menu.style.left = `${Math.max(8, Math.min(right + width <= innerWidth - 8 ? right : left, innerWidth - width - 8))}px`;
    menu.style.top = `${Math.max(8, Math.min(box.top, innerHeight - height - 8))}px`;
  }
  function showMenu(uids, anchor, focus = true) {
    if (!available || busy || anchor.getAttribute('aria-disabled') === 'true') return;
    menuEditing = false;
    menuUids = [...new Set(uids)].filter(uid => row(uid) && !row(uid).pending);
    if (!menuUids.length) return;
    menuAnchor?.setAttribute('aria-expanded', 'false'); menuAnchor = anchor;
    anchor.setAttribute('aria-expanded', 'true'); paintMenu(); menu.hidden = false;
    positionMenu();
    if (focus) (menu.querySelector('[aria-checked="true"]') || menu.firstElementChild)?.focus();
  }
  function closeMenu() { menu.hidden = true; menuEditing = false; menuAnchor?.setAttribute('aria-expanded', 'false'); menuAnchor = null; menuUids = []; }
  function escapeMenu() { if (menu.hidden) return false; const anchor = menuAnchor; closeMenu(); anchor?.focus(); return true; }
  menu.onkeydown = event => {
    if (event.key === 'ArrowLeft' || event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); escapeMenu(); }
    else if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
      event.preventDefault(); const buttons = [...menu.querySelectorAll('button')], index = buttons.indexOf(document.activeElement);
      buttons[event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1 : (index + (event.key === 'ArrowDown' ? 1 : buttons.length - 1)) % buttons.length]?.focus();
    }
  };
  const trigger = $('#item-menu [data-act="group"]');
  trigger.onpointerenter = event => { if (event.pointerType === 'mouse') showMenu([menuUid], trigger, false); };
  trigger.onkeydown = event => { if (event.key === 'ArrowRight') { event.preventDefault(); showMenu([menuUid], trigger); } };
  $('#item-menu').addEventListener('pointerover', event => { if (event.target.closest('button[data-act]')?.dataset.act !== 'group') closeMenu(); });
  $('#side-pick-group').onclick = event => showMenu([...pickedSessions], event.currentTarget);
  document.addEventListener('pointerdown', event => { if (!event.target.closest('#session-group-menu, #item-menu, #side-pick-group')) closeMenu(); }, true);
  addEventListener('resize', closeMenu);
  // Catalog writes publish a list invalidation on the UI event channel, which
  // refreshes immediately. A Hub can still merge a catalog edited directly on
  // a node without one, so keep a slow read while events flow and the old
  // 10 s cadence while they are down. Page sleep and hidden tabs read nothing.
  let refreshedAt = Date.now();
  const due = () => !document.hidden && !busy && !editing && !menuEditing && !SessionDockNetwork.paused;
  const reread = () => { refreshedAt = Date.now(); void refresh(); };
  void refresh();
  setInterval(() => {
    if (due() && Date.now() - refreshedAt >= (typeof uiEventsReady !== 'undefined' && uiEventsReady ? 60000 : 10000)) reread();
  }, 10000);
  // A change seen while editing is read on the next tick after editing ends.
  addEventListener('sessiondock-ui-sessions', () => { if (due()) reread(); else refreshedAt = 0; });
  addEventListener('sessiondock-network-resumed', () => { if (due()) reread(); });
  document.addEventListener('visibilitychange', () => { if (due()) reread(); });
  return {matches, paintRow, paintPickBar, paintHeading, paintSidebar, showMenu, closeMenu, escapeMenu,
    contains: name => catalog.includes(name), get names() { return catalog; }, get available() { return available; }};
})();
