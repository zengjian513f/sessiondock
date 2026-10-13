/* Top-level work adjacency. All writes go through the receiving session's node. */
(() => {
  'use strict';
  const $ = selector => document.querySelector(selector);
  const esc = value => String(value ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;', '<':'&lt;', '>':'&gt;', '"':'&quot;', "'":'&#39;'}[c]));
  const uid = new URLSearchParams(location.search).get('uid') || '';
  const kinds = {continues:'接续', branches:'任务分支', references:'引用'};
  let data, candidates = [], direction = 'predecessor', sequence = 0, saving = false;
  const infoUrl = target => `session-info.html?${new URLSearchParams({uid: target})}`;
  const conversationUrl = target => `./?${new URLSearchParams({sid: target})}`;
  async function request(path, body) {
    const response = await fetch(path, body ? {method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify({...body, _build:document.querySelector('meta[name="sessiondock-build"]').content})} : {});
    const result = await response.json();
    if (!response.ok) throw new Error(result.error || `请求失败（${response.status}）`);
    return result;
  }
  function card(item, side) {
    const direct = item.distance === 1;
    const label = item.title || item.sid || item.session_uid;
    const title = item.available ? `<a class="si-relation-title t" title="${esc(label)}" href="${esc(infoUrl(item.uid))}">${esc(label)}</a>` : `<span class="si-relation-title t">${esc(label)}</span>`;
    const meta = `${item.node_name || item.node_id || '本机'} · ${kinds[item.kind] || item.kind} · ${direct ? '直接' : `间接 ${item.distance} 层`}`;
    return `<article class="si-relation sd-session-card" data-uid="${esc(item.uid)}" data-side="${side}">
      <span class="ico">${SessionDockCard.icon(item.source)}</span><div class="body">
      ${title}
      <div class="m" title="${esc(meta)}">${esc(meta)}${(item.path_status || item.status) === 'candidate' ? ' · <span class="si-candidate">待核实</span>' : ''}${item.available ? '' : ' · 会话不可用'}</div>
      <details class="si-more"><summary aria-label="关系详情与操作" title="关系详情与操作"><svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="5" cy="12" r="1.5"/><circle cx="12" cy="12" r="1.5"/><circle cx="19" cy="12" r="1.5"/></svg></summary>
      <div class="si-relation-detail">
      <div class="cwd card-path">${esc(item.source)} · ${esc(item.sid || item.session_uid)}</div>
      ${item.artifact ? `<div class="si-artifact">${esc(item.artifact)}</div>` : ''}
      ${item.evidence ? `<details class="si-evidence"><summary>关系依据</summary><pre>${esc(item.evidence)}</pre></details>` : ''}
      <div class="si-actions">${item.available ? `<a href="${esc(conversationUrl(item.uid))}">打开会话</a>` : ''}
      ${direct ? `${item.status === 'candidate' ? '<button type="button" data-confirm>确认关系</button>' : ''}<button type="button" data-remove>移除关系</button>` : ''}</div></div></details></div></article>`;
  }
  function render() {
    const row = data.session;
    document.title = `${row.title || row.sid} · 会话信息`;
    $('#info-open').href = conversationUrl(uid); $('#info-open').hidden = false;
    const facts = [['机器', row.node_name || row.node_id || '本机'], ['Agent', row.source], ['UUID', row.sid], ['目录', row.cwd || '—'], ['创建时间', row.created || '—'], ['更新时间', row.updated || '—']];
    $('#info-summary').innerHTML = `<h2>${esc(row.title || row.sid)}</h2><dl class="si-facts">${facts.map(([k,v]) => `<div><dt>${k}</dt><dd>${esc(v)}</dd></div>`).join('')}</dl>`;
    $('#info-summary').hidden = false; $('#info-relations').hidden = false; $('#info-hints').hidden = !row.relations_top_level;
    for (const [side, all, direct] of [['predecessor', data.ancestors, data.predecessors], ['successor', data.descendants, data.successors]]) {
      $(`#${side}-count`).textContent = `${all.length} · 直接 ${direct.length}`;
      $(`#info-${side}s`).innerHTML = all.length ? all.map(item => card(item, side)).join('') : '<p class="si-empty">尚无已记录的关系</p>';
      $(`#add-${side}`).disabled = !row.relations_top_level;
    }
    $('#info-status').textContent = !row.relations_top_level ? '此会话不是顶级会话，前后序关系仅用于顶级会话。'
      : data.partial ? '部分机器暂不可用；前后序列表可能不完整。已保存的关系会保留。' : '';
  }
  async function refresh() {
    const version = ++sequence;
    $('#info-refresh').disabled = true;
    try {
      if (!uid) throw new Error('链接缺少会话标识。');
      const next = await request(`api/session/info?${new URLSearchParams({uid})}`);
      if (version !== sequence) return;
      data = next; render();
      SessionDockResearch.load(data.session);
    } catch (error) { if (version === sequence) $('#info-status').textContent = `读取失败：${error.message}`; }
    finally { if (version === sequence) $('#info-refresh').disabled = false; }
  }
  function choices() {
    const search = $('#relation-search').value.trim().toLocaleLowerCase();
    const selected = $('#relation-target').value;
    const rows = candidates.filter(row => row.uid !== uid && (!search || [row.title,row.cwd,row.sid,row.node_name,row.uid].join(' ').toLocaleLowerCase().includes(search)));
    $('#relation-target').innerHTML = '<option value="">请选择会话</option>' + rows.map(row => `<option value="${esc(row.uid)}">${esc(`${row.node_name || '本机'} · ${row.title || row.sid} · ${row.sid}`)}</option>`).join('');
    if (rows.some(row => row.uid === selected)) $('#relation-target').value = selected;
  }
  async function edit(side) {
    direction = side; $('#relation-form').reset(); $('#relation-error').textContent = '';
    $('#relation-form-title').textContent = side === 'predecessor' ? '添加前序' : '添加后序';
    $('#info-editor').hidden = false; $('#relation-search').focus();
    $('#relation-target').innerHTML = '<option value="">正在读取顶级会话…</option>';
    try { const result = await request('api/sessions/relations'); candidates = result.sessions || []; choices();
      if (result.partial) $('#relation-error').textContent = '部分机器暂不可用，仅列出当前可见的顶级会话。';
    } catch (error) { $('#relation-error').textContent = error.message; }
  }
  let hintsLoaded = false;
  $('#info-hints').ontoggle = async () => {
    if (!$('#info-hints').open || hintsLoaded) return;
    $('#hint-content').textContent = '正在读取开场消息…';
    try {
      const result = await request(`api/session/relations/hints?${new URLSearchParams({uid})}`);
      const messages = (result.messages || []).filter(item => item.documents.length || item.session_ids.length);
      $('#hint-content').innerHTML = messages.length ? messages.map(item => `<article class="si-relation"><p>消息 ${item.ordinal + 1}</p><pre>${esc(item.text)}</pre></article>`).join('') : '<p class="si-empty">前五条用户消息中未发现文档或 UUID 线索</p>';
      hintsLoaded = true;
    } catch (error) { $('#hint-content').textContent = `读取失败：${error.message}；收起后展开可重试。`; }
  };
  $('#info-refresh').onclick = () => { hintsLoaded = false; if ($('#info-hints').open) $('#info-hints').ontoggle(); refresh(); };
  $('#add-predecessor').onclick = () => edit('predecessor');
  $('#add-successor').onclick = () => edit('successor');
  $('#relation-cancel').onclick = () => { $('#info-editor').hidden = true; };
  $('#relation-search').oninput = choices;
  $('#relation-form').onsubmit = async event => {
    event.preventDefault(); if (saving) return;
    const target = $('#relation-target').value; if (!target) return;
    saving = true; $('#relation-save').disabled = true; $('#relation-error').textContent = '';
    try {
      await request('api/session/predecessor', {uid:direction === 'predecessor' ? uid : target, predecessor_uid:direction === 'predecessor' ? target : uid,
        kind:$('#relation-kind').value, status:$('#relation-state').value, artifact:$('#relation-artifact').value, evidence:$('#relation-evidence').value});
      $('#info-editor').hidden = true; await refresh();
    } catch (error) { $('#relation-error').textContent = `保存失败：${error.message}`; }
    finally { saving = false; $('#relation-save').disabled = false; }
  };
  $('#info-relations').onclick = async event => {
    const button = event.target.closest('[data-remove],[data-confirm]'); if (!button || saving) return;
    const card = button.closest('.si-relation'), previous = card.dataset.side === 'predecessor';
    const item = (previous ? data.predecessors : data.successors).find(row => row.uid === card.dataset.uid);
    if (!item) return;
    saving = true; button.disabled = true;
    try {
      await request('api/session/predecessor', {uid:previous ? uid : item.uid, predecessor_uid:previous ? item.uid : uid,
        kind:item.kind, status:'confirmed', artifact:item.artifact, evidence:item.evidence, remove:button.hasAttribute('data-remove')});
      await refresh();
    } catch (error) { $('#info-status').textContent = `保存失败：${error.message}`; }
    finally { saving = false; button.disabled = false; }
  };
  refresh();
})();
