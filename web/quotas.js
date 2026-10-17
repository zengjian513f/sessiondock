/* Account quotas are snapshots: no polling and no inferred reset to 100%. */
(async () => {
  'use strict';
  const root = document.querySelector('#quota-agents');
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
  function render() {
    const offsets = new Map([...root.querySelectorAll('.quota-scroll')].map(n => [n.dataset.source, n.scrollLeft]));
    root.replaceChildren();
    if (!nodes.length) return;
    for (const [source, name] of Object.entries(names)) {
      const section=el('section','si-panel quota-agent');section.dataset.source=source;
      const heading=el('h2','',name);heading.id=`quota-${source}-heading`;section.append(heading);
      const scroll=el('div','quota-scroll');scroll.dataset.source=source;
      scroll.tabIndex=0;scroll.setAttribute('role','region');scroll.setAttribute('aria-label',`${name} 额度表，可横向滚动`);
      const table=el('table','client-matrix quota-table');table.setAttribute('aria-labelledby',heading.id);
      const columns=el('colgroup');
      for (const cls of ['machine','group','window','remaining','reset','captured']) columns.append(el('col',`quota-col-${cls}`));
      table.append(columns);
      const head=el('thead'), labels=el('tr');
      for (const label of ['机器','额度组','窗口','剩余额度','下次重置','最后采集 / 状态']) {
        const cell=el('th','',label);cell.scope='col';labels.append(cell);
      }
      head.append(labels);table.append(head);
      for (const node of nodes) {
        const agent=(node.data?.agents || []).find(a => a.source===source);
        const buckets=agent?.buckets || [];
        const body=el('tbody','quota-node');body.dataset.node=node.id;
        for (const [index, bucket] of (buckets.length ? buckets : [null]).entries()) {
          const row=el('tr');
          if (index===0) {
            const machine=el('th','quota-machine-name',node.name);machine.scope='rowgroup';machine.rowSpan=Math.max(1,buckets.length);row.append(machine);
          }
          if (bucket) {
            row.append(el('td','',bucket.group || '—'),el('td','',windowName(bucket.window)));
            const remaining=el('td','quota-remaining');
            const valid=typeof bucket.remaining_percent==='number' && Number.isFinite(bucket.remaining_percent);
            remaining.append(el('strong','',valid ? `剩余 ${Math.max(0,bucket.remaining_percent).toLocaleString(undefined,{maximumFractionDigits:1})}%` : '剩余额度未知'));
            if (valid) {
              const bar=el('progress');bar.max=100;bar.value=Math.max(0,Math.min(100,bucket.remaining_percent));
              bar.setAttribute('aria-label',`${node.name} ${bucket.group} ${windowName(bucket.window)} 剩余额度`);remaining.append(bar);
            }
            row.append(remaining);
            const reset=el('td','quota-time',time(bucket.resets_at));
            if (bucket.resets_at && Date.parse(bucket.resets_at)<=Date.now()) reset.append(el('div','si-caption','已过记录的重置时间，等待新快照'));
            row.append(reset);
          } else {
            const text = !agent ? (node.error ? '额度暂不可用' : '正在读取…') : agent.configured===false ? '未配置此 CLI 或采集目录' : source==='claude' ? '等待 Claude 会话传入额度快照' : '暂无额度，点击“刷新额度”查询';
            const empty=el('td','si-caption',text);empty.colSpan=3;row.append(empty,el('td','si-caption','—'));
          }
          if (index===0) {
            const captured=el('td','quota-time',agent?.captured_at ? time(agent.captured_at) : '尚未采集');captured.rowSpan=Math.max(1,buckets.length);
            const error=node.error ? `读取失败 · ${node.error}，可点击刷新重试` : agent?.error;
            if (error) captured.append(el('div','quota-error',error));
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
        try {const data=await request(base+(collect?'/refresh':''),collect?'POST':'GET');node.data=data;node.error=null;render();}
        catch(error) {failures++;node.error=error.message;render();}
      }));
      status.textContent=nodes.length ? (failures ? `${failures} 台机器读取失败；其余结果已显示。` : '快照已读取；不会自动刷新。') : '暂无已启用的机器';
    } catch(error) {status.textContent=`读取失败 · ${error.message}`;}
    finally {refresh.disabled=false;}
  }
  refresh.addEventListener('click',()=>load(true));
  await load();
})();
