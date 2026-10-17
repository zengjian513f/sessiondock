/* Account quotas are snapshots: no polling and no inferred reset to 100%. */
(async () => {
  'use strict';
  const root = document.querySelector('#quota-machines');
  const status = document.querySelector('#quota-status');
  const refresh = document.querySelector('#quota-refresh');
  const names = {codex:'Codex', claude:'Claude Code', agy:'Agy'};
  let nodes = [], hub = false;
  const el = (tag, cls, text) => {const n=document.createElement(tag);n.className=cls || '';if(text!=null)n.textContent=text;return n;};
  async function request(path, method='GET') {
    const response=await fetch(path,{method,cache:'no-store'});
    if(!response.ok) throw new Error(`HTTP ${response.status}`);
    return response.json();
  }
  function time(value) {
    if(!value) return '未知';
    const d=new Date(value);return Number.isFinite(d.getTime()) ? d.toLocaleString() : '未知';
  }
  function windowName(value) {
    if(typeof value==='number') return value%1440===0 ? `${value/1440} 天` : value%60===0 ? `${value/60} 小时` : `${value} 分钟`;
    return {five_hour:'5 小时',seven_day:'每周',weekly:'每周','5h':'5 小时',spend_limit:'消费限额'}[value] || value || '额度窗口';
  }
  function render(node, data, error) {
    const section=node.section;
    section.replaceChildren(el('h2','',node.name));
    if(error) section.append(el('p','quota-error',`读取失败 · ${error}，可点击刷新重试`));
    if(!data) return;
    const grid=el('div','quota-agents');
    for(const agent of data.agents || []) {
      const card=el('section','quota-agent');card.dataset.source=agent.source;
      card.append(el('h3','',names[agent.source] || agent.source));
      card.append(el('p','si-caption',`最后采集：${agent.captured_at ? time(agent.captured_at) : '尚未采集'}`));
      if(agent.error) card.append(el('p','quota-error',agent.error));
      if(!(agent.buckets || []).length) card.append(el('p','si-caption',agent.configured===false ? '未配置此 CLI 或采集目录' : agent.source==='claude' ? '等待 Claude 会话传入额度快照' : '暂无额度，点击“刷新额度”查询'));
      for(const bucket of agent.buckets || []) {
        const row=el('div','quota-bucket');
        row.append(el('div','quota-label',`${bucket.group || ''} · ${windowName(bucket.window)}`));
        const valid=typeof bucket.remaining_percent==='number' && Number.isFinite(bucket.remaining_percent);
        row.append(el('strong','quota-remaining',valid ? `剩余 ${Math.max(0,bucket.remaining_percent).toLocaleString(undefined,{maximumFractionDigits:1})}%` : '剩余额度未知'));
        if(valid) {const bar=el('progress');bar.max=100;bar.value=Math.max(0,Math.min(100,bucket.remaining_percent));bar.setAttribute('aria-label',`${bucket.group} ${windowName(bucket.window)} 剩余额度`);row.append(bar);}
        row.append(el('div','si-caption',`重置：${time(bucket.resets_at)}`));
        if(bucket.resets_at && Date.parse(bucket.resets_at)<=Date.now()) row.append(el('div','si-caption','已过记录的重置时间，等待新快照'));
        card.append(row);
      }
      grid.append(card);
    }
    section.append(grid);
  }
  async function load(collect=false) {
    refresh.disabled=true;status.textContent=collect?'正在查询额度…':'正在读取快照…';
    try {
      if(!nodes.length) {
        const data=await request('api/nodes');hub=data.mode==='hub';
        nodes=data.nodes || [];
        root.replaceChildren();
        for(const node of nodes) {node.section=el('section','si-panel quota-machine');root.append(node.section);node.section.append(el('h2','',node.name),el('p','si-caption','正在读取…'));}
      }
      let failures=0;
      await Promise.all(nodes.map(async node=>{
        const base=hub?`api/nodes/${encodeURIComponent(node.id)}/api/quotas`:'api/quotas';
        try {const data=await request(base+(collect?'/refresh':''),collect?'POST':'GET');node.data=data;render(node,data);}
        catch(error) {failures++;render(node,node.data,error.message);}
      }));
      status.textContent=nodes.length ? (failures ? `${failures} 台机器读取失败；其余结果已显示。` : '快照已读取；不会自动刷新。') : '暂无已启用的机器';
    } catch(error) {status.textContent=`读取失败 · ${error.message}`;}
    finally {refresh.disabled=false;}
  }
  refresh.addEventListener('click',()=>load(true));
  await load();
})();
