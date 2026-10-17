/* Account quotas are snapshots: no polling and no inferred reset to 100%. */
async function initQuotas(container) {
  'use strict';
  const root = container.querySelector('#quota-agents');
  const status = container.querySelector('#quota-status');
  const refresh = container.querySelector('#quota-refresh');
  const names = {codex:'Codex', claude:'Claude Code', agy:'Agy'};
  let nodes = [], hub = false;
  const el = (tag, cls, text) => {const n=document.createElement(tag);n.className=cls || '';if(text!=null)n.textContent=text;return n;};
  async function request(path, method='GET') {
    const response=await fetch(path,{method,cache:'no-store'});
    if(!response.ok) throw new Error(`HTTP ${response.status}`);
    return response.json();
  }
  function time(value) {
    if(!value) return 'N/A';
    const d=new Date(value);return Number.isFinite(d.getTime()) ? d.toLocaleString(undefined,{month:'2-digit',day:'2-digit',hour:'2-digit',minute:'2-digit'}) : 'N/A';
  }
  function windowName(value) {
    if(typeof value==='number') return value%1440===0 ? `${value/1440} 天` : value%60===0 ? `${value/60} 小时` : `${value} 分钟`;
    return {five_hour:'5 小时',seven_day:'每周',weekly:'每周','5h':'5 小时',spend_limit:'消费限额'}[value] || value || 'N/A';
  }
  function render() {
    const offsets = new Map([...root.querySelectorAll('.quota-scroll')].map(n => [n.dataset.source, n.scrollLeft]));
    root.replaceChildren();
    if (!nodes.length) return;
    for (const [source, name] of Object.entries(names)) {
      const installedNodes=nodes.filter(node => node.clients?.some(client => client.source===source && client.installed));
      if (!installedNodes.length) continue;
      const section=el('section','quota-agent');section.dataset.source=source;
      const heading=el('h2','',name);heading.id=`quota-${source}-heading`;section.append(heading);
      const scroll=el('div','quota-scroll');scroll.dataset.source=source;
      scroll.tabIndex=0;scroll.setAttribute('role','region');scroll.setAttribute('aria-label',`${name} 额度表，可横向滚动`);
      const table=el('table','client-matrix quota-table');table.setAttribute('aria-labelledby',heading.id);
      const columns=el('colgroup');
      for (const cls of ['machine','group','window','remaining','reset','captured']) columns.append(el('col',`quota-col-${cls}`));
      table.append(columns);
      const head=el('thead'), labels=el('tr');
      for (const label of ['机器','额度组','窗口','剩余','重置 / 到期','采集']) {
        const cell=el('th','',label);cell.scope='col';labels.append(cell);
      }
      head.append(labels);table.append(head);
      for (const node of installedNodes) {
        const agent=(node.data?.agents || []).find(a => a.source===source);
        const buckets=[...(agent?.buckets || [])];
        if (source==='claude') buckets.push({group:'Usage credits',remaining_text:null});
        const body=el('tbody','quota-node');body.dataset.node=node.id;
        for (const [index, bucket] of (buckets.length ? buckets : [null]).entries()) {
          const row=el('tr');
          if (index===0) {
            const machine=el('th','quota-machine-name',node.name);machine.scope='rowgroup';machine.rowSpan=Math.max(1,buckets.length);row.append(machine);
          }
          if (bucket) {
            row.append(el('td','',bucket.group || 'N/A'),el('td','',windowName(bucket.window)));
            const remaining=el('td','quota-remaining');
            const valid=typeof bucket.remaining_percent==='number' && Number.isFinite(bucket.remaining_percent);
            remaining.append(el('strong','',valid ? `${Math.max(0,bucket.remaining_percent).toLocaleString(undefined,{maximumFractionDigits:1})}%` : (bucket.remaining_text ?? 'N/A')));
            if (valid) {
              const bar=el('progress');bar.max=100;bar.value=Math.max(0,Math.min(100,bucket.remaining_percent));
              bar.setAttribute('aria-label',`${node.name} ${bucket.group} ${windowName(bucket.window)} 剩余额度`);remaining.append(bar);
            }
            row.append(remaining);
            const reset=el('td','quota-time',time(bucket.resets_at || bucket.expires_at));
            if (bucket.resets_at || bucket.expires_at) reset.title=(bucket.expires_at ? '已返回卡片的最近到期：' : '')+new Date(bucket.resets_at || bucket.expires_at).toLocaleString();
            if (bucket.resets_at && Date.parse(bucket.resets_at)<=Date.now()) reset.title='已过重置时间，等待新快照';
            row.append(reset);
          } else {
            for (let i=0;i<4;i++) row.append(el('td','','N/A'));
          }
          if (index===0) {
            const captured=el('td','quota-time',agent?.captured_at ? time(agent.captured_at) : 'N/A');if (agent?.captured_at) captured.title=new Date(agent.captured_at).toLocaleString();captured.rowSpan=Math.max(1,buckets.length);
            const error=node.error ? `读取失败 · ${node.error}，可点击刷新重试` : agent?.error;
            if (error) { const warning=el('div','quota-error','查询失败');warning.title=error;captured.append(warning); }
            row.append(captured);
          }
          body.append(row);
        }
        table.append(body);
      }
      scroll.append(table);section.append(scroll);root.append(section);
      scroll.scrollLeft=offsets.get(source) || 0;
    }
  }
  async function load(collect=false) {
    refresh.disabled=true;status.textContent=collect?'正在查询额度…':'正在读取快照…';
    try {
      if(!nodes.length) {
        const data=await request('api/nodes');hub=data.mode==='hub';
        nodes=data.nodes || [];
        render();
      }
      let failures=0;
      await Promise.all(nodes.map(async node=>{
        const base=hub?`api/nodes/${encodeURIComponent(node.id)}/api/quotas`:'api/quotas';
        try {
          const [quotas,clients]=await Promise.allSettled([
            request(base+(collect?'/refresh':''),collect?'POST':'GET'),
            request(base.replace(/quotas$/, 'clients'))
          ]);
          if(clients.status==='fulfilled') node.clients=clients.value.clients || [];
          if(quotas.status==='rejected') throw quotas.reason;
          node.data=quotas.value;node.error=null;render();
        }
        catch(error) {failures++;node.error=error.message;render();}
      }));
      status.textContent=nodes.length ? (failures ? `${failures} 台读取失败` : (root.children.length ? '' : 'N/A')) : 'N/A';
    } catch(error) {status.textContent=`读取失败 · ${error.message}`;}
    finally {refresh.disabled=false;}
  }
  refresh.addEventListener('click',()=>load(true));
  await load();
}
(() => {
  const entry=document.querySelector('#quota-open');
  if (!entry) { initQuotas(document);return; }
  entry.addEventListener('click', event => {
    event.preventDefault();
    const dialog=document.createElement('dialog');
    dialog.id='quota-dialog';dialog.className='app-dialog';
    dialog.setAttribute('aria-labelledby','quota-title');
    dialog.innerHTML='<div class="quota-panel"><div class="modal-head"><h2 id="quota-title">Agent 额度</h2><button id="quota-refresh" class="btn" type="button">刷新</button><button class="modal-close" type="button" aria-label="关闭">×</button></div><div class="quota-body"><p id="quota-status" role="status"></p><div id="quota-agents"></div></div></div>';
    dialog.querySelector('.modal-close').onclick=()=>dialog.close();
    dialog.addEventListener('click',e=>{if(e.target===dialog) dialog.close();});
    dialog.addEventListener('close',()=>dialog.remove(),{once:true});
    document.body.append(dialog);dialog.showModal();
    initQuotas(dialog);
  });
})();
