'use strict';

// Catalogs persist on nodes; the Hub distributes their union. Browser storage
// holds only the current filter, never labels or assignments.
globalThis.SessionDockLabels = (() => {
  let catalog = {labels: [], groups: []}, available = false, refreshTask = null;
  let selected = [], busy = false;
  const dirtyLabels = new Map();
  let labelFilter = new Set(store.get('labelFilter', []));
  let groupFilter = store.get('groupFilter', '');
  const dialog = $('#session-label-dialog'), note = $('#session-label-note');
  $('#view [data-v="group"]').hidden = !SessionDockCapabilities.allows('metadata');
  if (!SessionDockCapabilities.allows('metadata') && S.view === 'group') {
    S.view = 'tree'; renderView(); renderSide();
  }
  const names = values => [...new Set(values)].sort((a, b) => a.localeCompare(b));
  const row = uid => indexedSessions().byUid.get(uid);
  function absorb(data) {
    catalog = {labels: names(data.labels || []), groups: names(data.groups || [])};
    const first = !available;
    available = true;
    paintFilters();
    if (first && (labelFilter.size || groupFilter)) renderSide();
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
    refreshTask = request('api/labels').then(absorb).catch(() => {}).finally(() => { refreshTask = null; });
    return refreshTask;
  }
  function matches(session) {
    if (!available) return true;
    return (!labelFilter.size || (session.labels || []).some(name => labelFilter.has(name)))
      && (!groupFilter || session.group === groupFilter);
  }
  function applyFilter() {
    store.set('labelFilter', [...labelFilter]);
    store.set('groupFilter', groupFilter);
    paintFilters(); renderSide();
  }
  function paintFilters() {
    const host = $('#session-label-filters');
    host.hidden = !available || (!catalog.labels.length && !catalog.groups.length);
    const chips = $('#session-label-chips');
    const key = JSON.stringify([catalog.labels, [...labelFilter]]);
    if (chips.dataset.key !== key) {
      chips.dataset.key = key;
      chips.replaceChildren();
      if (catalog.labels.length) {
        const all = document.createElement('button');
        all.type = 'button'; all.textContent = '全部标签'; all.dataset.all = '1';
        all.classList.toggle('on', !labelFilter.size);
        all.setAttribute('aria-pressed', String(!labelFilter.size));
        chips.append(all);
      }
      for (const name of catalog.labels) {
        const button = document.createElement('button');
        button.type = 'button'; button.textContent = name; button.dataset.label = name;
        button.classList.toggle('on', labelFilter.has(name));
        button.setAttribute('aria-pressed', String(labelFilter.has(name)));
        button.title = '点击选择或取消；右键或长按只选这个标签';
        chips.append(button);
      }
    }
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
    let badges = body.querySelector('.session-row-labels');
    if (!badges) { badges = document.createElement('div'); badges.className = 'session-row-labels'; body.append(badges); }
    const values = [...(session.group ? [`分组：${session.group}`] : []), ...(session.labels || [])];
    const key = JSON.stringify(values);
    if (badges.dataset.key === key) return;
    badges.dataset.key = key;
    badges.replaceChildren(...values.map(name => {
      const badge = document.createElement('span'); badge.textContent = name; return badge;
    }));
  }
  function paintPickBar(attaching) {
    const button = $('#side-pick-labels');
    button.hidden = attaching || !available;
    const count = [...pickedSessions].filter(uid => row(uid) && !row(uid).pending).length;
    button.disabled = !count || busy;
  }
  function paintOptions() {
    const host = $('#session-label-options');
    host.replaceChildren();
    for (const name of catalog.labels) {
      const label = document.createElement('label'), input = document.createElement('input');
      input.type = 'checkbox'; input.dataset.label = name;
      const count = selected.filter(uid => (row(uid)?.labels || []).includes(name)).length;
      input.checked = dirtyLabels.has(name) ? dirtyLabels.get(name) : count === selected.length;
      input.indeterminate = !dirtyLabels.has(name) && count > 0 && count < selected.length;
      input.onchange = () => { dirtyLabels.set(name, input.checked); };
      label.append(input, document.createTextNode(name)); host.append(label);
    }
  }
  function paintGroup(value) {
    const select = $('#session-label-group');
    select.replaceChildren(new Option('不分组', ''), ...catalog.groups.map(name => new Option(name, name)));
    if (selected.length > 1) select.prepend(new Option('保持各会话原分组', ''));
    if (value === null && selected.length > 1) select.selectedIndex = 0;
    else select.selectedIndex = [...select.options].findIndex((option, index) => option.value === value && (selected.length === 1 || index > 0));
  }
  function setBusy(value) {
    busy = value;
    for (const element of $('#session-label-form').querySelectorAll('button, input, select')) element.disabled = value;
    paintPickBar(!!S.nestAttach);
  }
  async function open(uids) {
    selected = [...new Set(uids)].filter(uid => row(uid) && !row(uid).pending);
    if (!selected.length || busy) return;
    dirtyLabels.clear(); note.textContent = '读取标签…';
    $('#session-label-summary').textContent = selected.length === 1 ? row(selected[0]).title : `已选 ${selected.length} 个会话；未改动的标签和分组保持原样`;
    $('#session-label-new').value = ''; $('#session-group-new').value = '';
    dialog.showModal(); setBusy(true);
    await refresh();
    paintOptions(); paintGroup(selected.length === 1 ? row(selected[0]).group || '' : null);
    note.textContent = available ? '' : '标签读取失败，请关闭后重试。';
    setBusy(false);
    $('#session-label-save').disabled = !available;
  }
  async function create(kind) {
    if (busy) return;
    const input = $(kind === 'labels' ? '#session-label-new' : '#session-group-new');
    const name = input.value.trim();
    if (!name) { note.textContent = '请输入名称'; input.focus(); return; }
    setBusy(true);
    try {
      const data = await request('api/labels', {[kind]: [name]});
      absorb(data);
      if (kind === 'labels') { dirtyLabels.set(name, true); paintOptions(); }
      else { paintGroup(name); }
      input.value = '';
      note.textContent = data.sync_errors?.length ? '已保存；部分节点将在恢复连接后同步。' : '已创建';
    } catch (error) { note.textContent = error.message; }
    finally { setBusy(false); }
  }
  function applyAssignment(uid, data) {
    for (const session of [...S.sessions, ...(S.results || [])]) if (session.uid === uid) {
      session.labels = data.labels || []; session.group = data.group || null;
    }
    for (const entry of cache.values()) if (entry.meta.uid === uid && !entry.meta.agent_id) {
      entry.meta.labels = data.labels || []; entry.meta.group = data.group || null;
    }
  }
  $('#session-label-form').onsubmit = async event => {
    event.preventDefault(); if (busy) return;
    const add_labels = [], remove_labels = [];
    for (const [name, checked] of dirtyLabels) (checked ? add_labels : remove_labels).push(name);
    const groupSelect = $('#session-label-group'), group = groupSelect.value;
    const body = {add_labels, remove_labels, set_group: selected.length === 1 || groupSelect.selectedIndex !== 0, group: group || null};
    setBusy(true); note.textContent = '保存中…';
    const failed = [];
    try {
      // Cross-node batches route each scoped UID to its owning node.
      for (const uid of selected) {
        try { applyAssignment(uid, await request('api/session/labels', {uid, ...body})); }
        catch (error) { failed.push(`${row(uid)?.title || uid}：${error.message}`); }
      }
      await loadSessions(true); renderSide();
      if (failed.length) note.textContent = `部分保存失败：${failed.join('；')}`;
      else { dialog.close(); void refresh(); }
    } finally { setBusy(false); }
  };
  for (const id of ['session-label-close', 'session-label-cancel']) $(`#${id}`).onclick = () => { if (!busy) dialog.close(); };
  dialog.addEventListener('cancel', event => { if (busy) event.preventDefault(); });
  $('#session-label-create').onclick = () => create('labels');
  $('#session-group-create').onclick = () => create('groups');
  for (const [id, kind] of [['session-label-new', 'labels'], ['session-group-new', 'groups']]) {
    $(`#${id}`).onkeydown = event => { if (event.key === 'Enter') { event.preventDefault(); void create(kind); } };
  }
  $('#side-pick-labels').onclick = () => open([...pickedSessions]);
  $('#session-group-filter').onchange = event => { groupFilter = event.target.value; applyFilter(); };
  const chips = $('#session-label-chips');
  let press = null, suppress = null;
  function cancelHold() { clearTimeout(press?.timer); press = null; }
  function only(button) { labelFilter = new Set([button.dataset.label]); applyFilter(); }
  chips.oncontextmenu = event => {
    const button = event.target.closest('button[data-label]');
    if (button) { event.preventDefault(); only(button); }
  };
  chips.onpointerdown = event => {
    const button = event.target.closest('button[data-label]');
    if (!button || event.pointerType === 'mouse' || event.button !== 0) return;
    cancelHold();
    press = {x: event.clientX, y: event.clientY, timer: setTimeout(() => {
      press = null; suppress = button.dataset.label; only(button);
      setTimeout(() => { suppress = null; }, 800);
    }, LONG_PRESS_MS)};
  };
  chips.onpointermove = event => {
    if (press && (Math.abs(event.clientX - press.x) > LONG_PRESS_SLOP || Math.abs(event.clientY - press.y) > LONG_PRESS_SLOP)) cancelHold();
  };
  for (const type of ['pointerup', 'pointercancel', 'pointerleave', 'scroll']) chips.addEventListener(type, cancelHold);
  chips.onclick = event => {
    const button = event.target.closest('button'); if (!button) return;
    const name = button.dataset.label;
    if (name && name === suppress) { suppress = null; return; }
    if (!name) labelFilter.clear();
    else if (labelFilter.has(name)) labelFilter.delete(name); else labelFilter.add(name);
    applyFilter();
  };
  void refresh();
  setInterval(() => { if (!document.hidden && !dialog.open) void refresh(); }, 10000);
  return {open, matches, paintRow, paintPickBar, get available() { return available; }};
})();
