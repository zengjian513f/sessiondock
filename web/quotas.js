/* Read the shared snapshot first; automatic refresh reuses the node cache. */
async function initQuotas(container) {
  'use strict';
  const root = container.querySelector('#quota-agents');
  const status = container.querySelector('#quota-status');
  const refresh = container.querySelector('#quota-refresh');
  const names = {codex:'Codex', claude:'Claude Code', agy:'Agy'};
  let nodes = [], hub = false, stopped = false;
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
  function countdown(value) {
    const end=Date.parse(value);
    if (!Number.isFinite(end)) return 'N/A';
    let seconds=Math.max(0,Math.ceil((end-Date.now())/1000));
    if (!seconds) return '已到期';
    const days=Math.floor(seconds/86400);seconds%=86400;
    const hours=Math.floor(seconds/3600);seconds%=3600;
    const minutes=Math.floor(seconds/60);
    return (days ? days+'d ' : '')+String(hours).padStart(2,'0')+':'+String(minutes).padStart(2,'0');
  }
  function deadline(value, expiry=false) {
    const cell=el('td','quota-time',countdown(value));
    if (value) {
      cell.dataset.deadline=value;
      cell.title=(expiry ? '已返回卡片的最近到期：' : '')+new Date(value).toLocaleString();
    }
    return cell;
  }
  function tick() {
    for (const cell of root.querySelectorAll('[data-deadline]')) cell.textContent=countdown(cell.dataset.deadline);
  }
  function render() {
    const offsets = new Map([...root.querySelectorAll('.quota-scroll')].map(n => [n.dataset.source, n.scrollLeft]));
    root.replaceChildren();
    for (const [source, name] of Object.entries(names)) {
      const installedNodes=nodes.filter(node => node.clients?.some(client => client.source===source && client.installed));
      if (!installedNodes.length) continue;
      const grouped=source==='agy', codex=source==='codex';
      const section=el('section','quota-agent');section.dataset.source=source;
      const heading=el('h2','',name);heading.id=`quota-${source}-heading`;section.append(heading);
      const scroll=el('div','quota-scroll');scroll.dataset.source=source;
      scroll.tabIndex=0;scroll.setAttribute('role','region');scroll.setAttribute('aria-label',`${name} 额度表，可横向滚动`);
      const table=el('table','client-matrix quota-table');table.setAttribute('aria-labelledby',heading.id);
      const fields=[['machine','机器'],...(grouped ? [['group','额度组']] : []),['window','窗口'],['remaining','已使用'],['reset','重置倒计时'],
        ...(codex ? [['credits','Credits'],['cards','重置卡']] : []),['captured','采集']];
      const columns=el('colgroup'), head=el('thead'), labels=el('tr');
      for (const [key,label] of fields) {
        columns.append(el('col',`quota-col-${key}`));
        const cell=el('th',key==='credits'?'quota-credits':'',label);cell.scope='col';labels.append(cell);
      }
      head.append(labels);table.append(columns,head);
      for (const node of installedNodes) {
        const agent=(node.data?.agents || []).find(a => a.source===source);
        const all=agent?.buckets || [];
        const credits=all.filter(b=>b.id==='credits' || b.id?.endsWith(':credits'));
        const cards=all.find(b=>b.id==='reset-credits');
        const buckets=codex ? all.filter(b=>!credits.includes(b) && b!==cards) : all;
        const rows=buckets.length ? buckets : [null];
        const body=el('tbody','quota-node');body.dataset.node=node.id;
        for (const [index, bucket] of rows.entries()) {
          const row=el('tr');
          if (index===0) {
            const machine=el('th','quota-machine-name',node.name);machine.scope='rowgroup';machine.rowSpan=rows.length;row.append(machine);
          }
          if (grouped) row.append(el('td','',bucket?.group || 'N/A'));
          let window=windowName(bucket?.window);
          if (codex && bucket?.group && bucket.group.toLowerCase()!=='codex') window=bucket.group+' · '+window;
          row.append(el('td','',window));
          const remaining=el('td','quota-remaining');
          const valid=typeof bucket?.remaining_percent==='number' && Number.isFinite(bucket.remaining_percent);
          if (valid) {
            const used=Math.max(0,100-bucket.remaining_percent);
            const value=Math.min(100,used);
            const gauge=el('span','sdk-gauge');gauge.classList.toggle('low',Math.round(value)>=80);
            const bar=el('i','sdk-bar');bar.setAttribute('role','progressbar');
            bar.setAttribute('aria-label',`${node.name} ${window} 已使用额度`);
            bar.setAttribute('aria-valuemin','0');bar.setAttribute('aria-valuemax','100');
            bar.setAttribute('aria-valuenow',String(value));
            const fill=el('u');fill.style.width=`${value}%`;bar.append(fill);
            gauge.append(bar,el('b','',`${used.toLocaleString(undefined,{maximumFractionDigits:1})}%`));
            remaining.append(gauge);
          } else remaining.append(el('b','',bucket?.remaining_text ?? 'N/A'));
          row.append(remaining,deadline(bucket?.resets_at));
          if (index===0) {
            if (codex) {
              const balance=el('td','quota-credits');balance.rowSpan=rows.length;
              for (const credit of (credits.length ? credits : [null])) {
                const value=credit?.remaining_text;
                const number=(typeof value==='number' || (typeof value==='string' && value.trim())) ? Number(value) : NaN;
                balance.append(el('div','',Number.isFinite(number) ? Math.trunc(number).toLocaleString('en-US') : (value ?? 'N/A')));
                if (credits.length>1) balance.lastChild.title=credit.group || '';
              }
              const card=el('td','quota-cards',cards?.remaining_text ?? 'N/A');card.rowSpan=rows.length;
              if (cards?.expires_at) {
                const expiry=deadline(cards.expires_at,true);
                const detail=el('div','quota-card-expiry');
                detail.textContent=expiry.textContent;detail.dataset.deadline=cards.expires_at;detail.title=expiry.title;
                card.append(detail);
              }
              row.append(balance,card);
            }
            const captured=el('td','quota-time',time(agent?.captured_at));
            if (agent?.captured_at) captured.title=new Date(agent.captured_at).toLocaleString();
            captured.rowSpan=rows.length;
            const error=node.error || agent?.error;
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
    refresh.disabled=true;status.textContent='正在查询额度…';
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
          if(quotas.status==='fulfilled') {node.data=quotas.value;node.error=null;}
          else if(collect) throw quotas.reason;
          else node.error=quotas.reason.message;
          render();
          // Each node refreshes after its own cached read, without waiting for peers.
          if(!collect && !stopped) {
            node.data=await request(base+'/refresh?cached=true','POST');node.error=null;render();
          }
        }
        catch(error) {failures++;node.error=error.message;render();}
      }));
      status.textContent=nodes.length ? (failures ? `${failures} 台读取失败` : (root.children.length ? '' : 'N/A')) : 'N/A';
    } catch(error) {status.textContent=`读取失败 · ${error.message}`;}
    finally {refresh.disabled=false;}
  }
  refresh.addEventListener('click',()=>load(true));
  // Only update the local clock; opening and the refresh button collect quotas.
  const timer=setInterval(tick,1000);
  const stopClock=()=>{stopped=true;clearInterval(timer);window.removeEventListener('pagehide',stopClock);};
  container.addEventListener('close',stopClock,{once:true});
  window.addEventListener('pagehide',stopClock,{once:true});
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
