/* Research stays owned by the Labdesk ledger; never infer links from history. */
(() => {
  'use strict';
  const $ = selector => document.querySelector(selector);
  const esc = value => String(value ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;', '<':'&lt;', '>':'&gt;', '"':'&quot;', "'":'&#39;'}[c]));
  const base = globalThis.SessionDockCapabilities?.integration('labdesk');
  let current, controller, generation = 0, rendered = false;
  const labels = {
    unregistered: '此会话尚未登记到研究台账。',
    node_unmapped: '研究台账尚未配置此会话所在机器。',
    host_mismatch: '台账记录的机器与当前会话不同，暂不显示关联。',
  };
  function url(path, params) {
    const target = new URL(path, base);
    target.search = new URLSearchParams(Object.entries(params).filter(([,v]) => v != null && v !== ''));
    return esc(target.href);
  }
  function link(href, title, available = true) {
    return available ? `<a class="t" href="${href}" target="_blank" rel="noopener">${esc(title)}</a>` : `<span class="t">${esc(title)}</span>`;
  }
  const title = (id, name) => name && name !== id ? `${id} · ${name}` : id;
  function overflow(items, label) {
    return items.slice(0, 5).join('') + (items.length > 5
      ? `<details class="si-research-overflow"><summary>展开其余 ${items.length - 5} ${label}</summary>${items.slice(5).join('')}</details>` : '');
  }
  function experiment(item) {
    const steps = item.steps.map(step => `<article class="sd-session-card si-research-row si-research-step"><div class="body">
      ${link(url('experiments', {project:item.project, eid:item.experiment_id, step:step.step_id}), title(step.step_id, step.name), item.available)}
      <div class="m card-meta">${esc(step.status || '状态未记录')} · 会话直接关联</div></div></article>`);
    const row = `<div class="body">${link(url('experiments', {project:item.project, eid:item.experiment_id}), title(item.experiment_id, item.name), item.available)}
      <div class="m card-meta">${esc([item.project, item.status, item.direct ? '会话直接关联' : '通过步骤关联', !item.available ? '实验记录不可用' : ''].filter(Boolean).join(' · '))}</div></div>`;
    return steps.length ? `<details class="si-research-experiment" open><summary class="sd-session-card si-research-row">${row}<span class="si-caption">${steps.length} 步骤</span></summary><div class="si-research-steps">${overflow(steps, '个步骤')}</div></details>`
      : `<article class="sd-session-card si-research-row si-research-experiment-row">${row}</article>`;
  }
  function report(item, direct) {
    const context = [item.project, direct ? '会话直接关联' : '通过实验关联', ...item.experiments.map(e => e.experiment_id), !item.available ? '研报记录不可用' : ''].filter(Boolean).join(' · ');
    return `<article class="sd-session-card si-research-row"><div class="body">${link(url('reports', {rid:item.report_id, project:item.project}), title(item.report_id, item.title), item.available)}<div class="m card-meta">${esc(context)}</div></div></article>`;
  }
  function render(value) {
    if (!['ok', ...Object.keys(labels)].includes(value.status) || !['experiments','reports','related_reports'].every(k => Array.isArray(value[k]))) throw new Error('研究台账返回了无效结果');
    const count = value.experiments.reduce((sum, item) => sum + item.steps.length, 0);
    const empty = !value.experiments.length && !value.reports.length;
    $('#research-count').textContent = value.status === 'ok' ? `实验 ${value.experiments.length} · 步骤 ${count} · 研报 ${value.reports.length}` : '';
    $('#research-content').innerHTML = (value.experiments.length ? `<h3 class="si-research-label">实验</h3>${overflow(value.experiments.map(experiment), '个实验')}` : '')
      + (value.reports.length ? `<h3 class="si-research-label">研报</h3>${overflow(value.reports.map(item => report(item, true)), '篇研报')}` : '')
      + (value.related_reports.length ? `<details class="si-research-related"><summary>实验内其他研报 · ${value.related_reports.length}</summary><p class="si-caption">以下研报与实验关联，未直接登记到本会话，不计入上方研报数量。</p>${overflow(value.related_reports.map(item => report(item, false)), '篇研报')}</details>` : '');
    $('#research-status').textContent = labels[value.status] || (empty ? '尚无已登记的研究关联。' : '');
    rendered = true;
  }
  async function load(row) {
    if (!base) { $('#info-research').hidden = true; return; }
    current = row;
    controller?.abort(); controller = new AbortController();
    const version = ++generation, signal = controller.signal;
    $('#info-research').hidden = false;
    $('#research-retry').hidden = true;
    $('#research-status').textContent = rendered ? '正在刷新研究关联…' : '正在读取研究关联…';
    const timer = setTimeout(() => controller?.signal === signal && controller.abort(), 15000);
    try {
      let node = row.node_id;
      if (!node) {
        const response = await fetch('api/meta', {signal});
        if (!response.ok) throw new Error('无法读取会话所在机器');
        node = (await response.json()).node_id;
      }
      if (!node || !row.source || !row.sid) throw new Error('缺少会话或机器标识');
      const sid = row.source === 'claude' && row.agent_id && !row.sid.includes('/agent:')
        ? `${row.sid}/agent:${row.agent_id}` : row.sid;
      const target = new URL('api/session-research', base);
      target.search = new URLSearchParams({session_uid:`${row.source}:${sid}`, node});
      const response = await fetch(target, {signal, credentials:'same-origin', headers:{Accept:'application/json'}});
      if (!response.ok) throw new Error(response.status === 401 || response.status === 403 ? '研究台账认证不可用' : `研究台账请求失败（${response.status}）`);
      const value = await response.json();
      if (version === generation) render(value);
    } catch (error) {
      if (version !== generation) return;
      const reason = signal.aborted ? '请求超时' : error.message;
      $('#research-status').textContent = `${rendered ? '刷新失败，以下保留上次结果' : '研究关联暂时无法加载'}：${reason}`;
      $('#research-retry').hidden = false;
    } finally { clearTimeout(timer); }
  }
  $('#research-retry').onclick = () => current && load(current);
  window.SessionDockResearch = {load};
})();
