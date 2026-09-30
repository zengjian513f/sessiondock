'use strict';

// Catalogs persist on nodes; the Hub distributes their union. Browser storage
// holds only the current filter, never groups or assignments.
globalThis.SessionDockGroups = (() => {
  let catalog = {groups: []}, available = false, refreshTask = null;
  let selected = [], busy = false;
  let groupFilter = store.get('groupFilter', '');
  const dialog = $('#session-group-dialog'), note = $('#session-group-note');
  $('#view [data-v="group"]').hidden = !SessionDockCapabilities.allows('metadata');
  if (!SessionDockCapabilities.allows('metadata') && S.view === 'group') {
    S.view = 'tree'; renderView(); renderSide();
  }
  const names = values => [...new Set(values)].sort((a, b) => a.localeCompare(b));
  const row = uid => indexedSessions().byUid.get(uid);
  function absorb(data) {
    catalog = {groups: names(data.groups || [])};
    const first = !available;
    available = true;
    paintFilters();
    if (first && groupFilter) renderSide();
    paintPickBar(!!S.nestAttach);
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
    if (!SessionDockCapabilities.allows('metadata')) return;
    if (refreshTask) return refreshTask;
    refreshTask = request('api/groups').then(absorb).catch(() => {}).finally(() => { refreshTask = null; });
    return refreshTask;
  }
  function matches(session) {
    if (!available) return true;
    return !groupFilter || session.group === groupFilter;
  }
  function applyFilter() {
    store.set('groupFilter', groupFilter);
    paintFilters(); renderSide();
  }
  function paintFilters() {
    const host = $('#session-group-filters');
    host.hidden = !available || !catalog.groups.length;
    const select = $('#session-group-filter');
    const groupKey = JSON.stringify([catalog.groups, groupFilter]);
    if (select.dataset.key !== groupKey) {
      select.dataset.key = groupKey;
      select.replaceChildren(new Option('全部分组', ''), ...catalog.groups.map(name => new Option(name, name)));
      select.value = groupFilter;
    }
  }
  function paintRow(node, session) {
    const body = node.querySelector('.body');
    if (!body) return;
    let badges = body.querySelector('.session-row-group');
    if (!badges) { badges = document.createElement('div'); badges.className = 'session-row-group'; body.append(badges); }
    badges.textContent = session.group ? `分组：${session.group}` : '';
  }
  function paintPickBar(attaching) {
    const button = $('#side-pick-group');
    button.hidden = attaching || !available;
    const count = [...pickedSessions].filter(uid => row(uid) && !row(uid).pending).length;
    button.disabled = !count || busy;
  }
  function paintGroup(value) {
    const select = $('#session-group-select');
    select.replaceChildren(new Option('不分组', ''), ...catalog.groups.map(name => new Option(name, name)));
    if (selected.length > 1) select.prepend(new Option('保持各会话原分组', ''));
    if (value === null && selected.length > 1) select.selectedIndex = 0;
    else select.selectedIndex = [...select.options].findIndex((option, index) => option.value === value && (selected.length === 1 || index > 0));
  }
  function setBusy(value) {
    busy = value;
    for (const element of $('#session-group-form').querySelectorAll('button, input, select')) element.disabled = value;
    paintPickBar(!!S.nestAttach);
  }
  async function open(uids) {
    selected = [...new Set(uids)].filter(uid => row(uid) && !row(uid).pending);
    if (!selected.length || busy) return;
    note.textContent = '读取分组…';
    $('#session-group-summary').textContent = selected.length === 1 ? row(selected[0]).title : `已选 ${selected.length} 个会话；可统一设置分组，或保持各会话原分组`;
    $('#session-group-new').value = '';
    dialog.showModal(); setBusy(true);
    await refresh();
    paintGroup(selected.length === 1 ? row(selected[0]).group || '' : null);
    note.textContent = available ? '' : '分组读取失败，请关闭后重试。';
    setBusy(false);
    $('#session-group-save').disabled = !available;
  }
  async function create() {
    if (busy) return;
    const input = $('#session-group-new');
    const name = input.value.trim();
    if (!name) { note.textContent = '请输入名称'; input.focus(); return; }
    setBusy(true);
    try {
      const data = await request('api/groups', {groups: [name]});
      absorb(data);
      paintGroup(name);
      input.value = '';
      note.textContent = data.sync_errors?.length ? '已保存；部分节点将在恢复连接后同步。' : '已创建';
    } catch (error) { note.textContent = error.message; }
    finally { setBusy(false); }
  }
  function applyAssignment(uid, data) {
    for (const session of [...S.sessions, ...(S.results || [])]) if (session.uid === uid) {
      session.group = data.group || null;
    }
    for (const entry of cache.values()) if (entry.meta.uid === uid && !entry.meta.agent_id) {
      entry.meta.group = data.group || null;
    }
  }
  $('#session-group-form').onsubmit = async event => {
    event.preventDefault(); if (busy) return;
    const groupSelect = $('#session-group-select'), group = groupSelect.value;
    const body = {set_group: selected.length === 1 || groupSelect.selectedIndex !== 0, group: group || null};
    setBusy(true); note.textContent = '保存中…';
    const failed = [];
    try {
      // Cross-node batches route each scoped UID to its owning node.
      for (const uid of selected) {
        try { applyAssignment(uid, await request('api/session/group', {uid, ...body})); }
        catch (error) { failed.push(`${row(uid)?.title || uid}：${error.message}`); }
      }
      await loadSessions(true); renderSide();
      if (failed.length) note.textContent = `部分保存失败：${failed.join('；')}`;
      else { dialog.close(); void refresh(); }
    } finally { setBusy(false); }
  };
  for (const id of ['session-group-close', 'session-group-cancel']) $(`#${id}`).onclick = () => { if (!busy) dialog.close(); };
  dialog.addEventListener('cancel', event => { if (busy) event.preventDefault(); });
  $('#session-group-create').onclick = () => create();
  $('#session-group-new').onkeydown = event => { if (event.key === 'Enter') { event.preventDefault(); void create(); } };
  $('#side-pick-group').onclick = () => open([...pickedSessions]);
  $('#session-group-filter').onchange = event => { groupFilter = event.target.value; applyFilter(); };
  void refresh();
  setInterval(() => { if (!document.hidden && !dialog.open) void refresh(); }, 10000);
  return {open, matches, paintRow, paintPickBar, get available() { return available; }};
})();
