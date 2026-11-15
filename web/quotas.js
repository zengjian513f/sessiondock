/* Read the shared snapshot first; automatic refresh reuses the node cache. */
async function initQuotas(container) {
  'use strict';
  const root = container.querySelector('#quota-agents');
  const status = container.querySelector('#quota-status');
  const refresh = container.querySelector('#quota-refresh');
  const tabs = container.querySelector('#quota-tabs');
  const viewport = container.querySelector('.quota-body');
  const cycles = initCycleUsage(()=>render());
  const names = {codex:'Codex', kiro:'Kiro', agy:'Agy', claude:'Claude Code', droid:'Droid'};
  let nodes = [], hub = false, stopped = false;
  let selected = 'codex';
  const positions = new Map(), buttons = new Map();
  const el = (tag, cls, text) => {const n=document.createElement(tag);n.className=cls || '';if(text!=null)n.textContent=text;return n;};
  function select(source, focus=false) {
    if(buttons.get(source)?.hidden)return;
    positions.set(selected,viewport.scrollTop);
    selected=source;
    for(const [key,button] of buttons) {
      const active=key===selected;
      button.setAttribute('aria-selected',String(active));button.tabIndex=active?0:-1;
      const panel=container.querySelector(`#quota-panel-${key}`);
      if(panel)panel.hidden=!active;
    }
    viewport.scrollTop=positions.get(selected) || 0;
    if(focus)buttons.get(source).focus({preventScroll:true});
  }
  for(const [source,name] of Object.entries(names)) {
    const button=el('button','quota-tab',name);
    button.type='button';button.id=`quota-tab-${source}`;button.hidden=true;
    button.setAttribute('role','tab');button.setAttribute('aria-controls',`quota-panel-${source}`);
    button.setAttribute('aria-selected',String(source===selected));button.tabIndex=source===selected?0:-1;
    button.onclick=()=>select(source);
    button.onkeydown=event=>{
      const keys=[...buttons.keys()].filter(key=>!buttons.get(key).hidden);
      let index=keys.indexOf(source);
      if(event.key==='ArrowRight')index=(index+1)%keys.length;
      else if(event.key==='ArrowLeft')index=(index+keys.length-1)%keys.length;
      else if(event.key==='Home')index=0;
      else if(event.key==='End')index=keys.length-1;
      else return;
      event.preventDefault();select(keys[index],true);
      buttons.get(keys[index]).scrollIntoView({block:'nearest',inline:'nearest'});
    };
    buttons.set(source,button);tabs.append(button);
  }
  // Keep the connected scrollers, rows and focused cells alive while values
  // change. Clearing the page briefly collapsed its height and clamped scrollTop.
  function update(current, next) {
    if(current.nodeType===Node.TEXT_NODE) {
      if(current.nodeValue!==next.nodeValue)current.nodeValue=next.nodeValue;
      return;
    }
    for(const attr of [...current.attributes])if(!next.hasAttribute(attr.name))current.removeAttribute(attr.name);
    for(const attr of next.attributes)if(current.getAttribute(attr.name)!==attr.value)current.setAttribute(attr.name,attr.value);
    const children=[...current.childNodes], used=new Set();
    const key=node=>node.nodeType===Node.ELEMENT_NODE ? node.dataset.key || node.id : null;
    for(const [index,child] of [...next.childNodes].entries()) {
      const identity=key(child);
      const old=identity ? children.find(node=>key(node)===identity) : children[index];
      const match=old && !used.has(old) && key(old)===identity && old.nodeType===child.nodeType && old.nodeName===child.nodeName;
      const target=match?old:child;
      if(match)update(old,child);
      used.add(target);
      if(current.childNodes[index]!==target)current.insertBefore(target,current.childNodes[index] || null);
    }
    for(const child of children)if(!used.has(child))child.remove();
  }
  async function request(path, method='GET') {
    const response=await fetch(path,{method,cache:'no-store'});
    if(!response.ok) throw new Error([502,503,504].includes(response.status)?'节点暂不可达':`读取失败（HTTP ${response.status}）`);
    return response.json();
  }
  function time(value) {
    if(!value) return 'N/A';
    const d=new Date(value);return Number.isFinite(d.getTime()) ? d.toLocaleString(undefined,{month:'2-digit',day:'2-digit',hour:'2-digit',minute:'2-digit'}) : 'N/A';
  }
  function windowName(value) {
    return value == null || value === '' ? 'N/A' : UsageUI.window(value);
  }
  function countdown(value) {
    return UsageUI.countdown(value);
  }
  function deadline(value, expiry=false) {
    const cell=el('td','quota-time',countdown(value));
    if (value) {
      cell.dataset.deadline=value;
      cell.dataset.expiry=String(expiry);
      cell.title=(expiry ? '已返回卡片的最近到期：' : `重置剩余 ${countdown(value)}；`)+new Date(value).toLocaleString();
    }
    return cell;
  }
  function tick() {
    for (const cell of root.querySelectorAll('[data-deadline]')) {
      const value=cell.dataset.deadline,label=countdown(value);
      if(cell.textContent!==label)cell.textContent=label;
      cell.title=(cell.dataset.expiry==='true'?'已返回卡片的最近到期：':`重置剩余 ${label}；`)+new Date(value).toLocaleString();
    }
  }
  function render() {
    if(stopped)return;
    const sources=Object.keys(names).filter(source=>nodes.some(node=>node.clients?.some(client=>client.source===source && client.installed)));
    const previous=selected;
    if(!sources.includes(selected))selected=sources[0] || selected;
    tabs.hidden=!sources.length;
    for(const [source,button] of buttons) {
      button.hidden=!sources.includes(source);button.tabIndex=source===selected?0:-1;
      button.setAttribute('aria-selected',String(source===selected));
    }
    const next=el('div');next.id=root.id;
    for (const [source, name] of Object.entries(names)) {
      const installedNodes=nodes.filter(node => node.clients?.some(client => client.source===source && client.installed));
      if (!installedNodes.length) continue;
      const grouped=['agy','droid'].includes(source), codex=source==='codex';
      const section=el('section','quota-agent');section.dataset.source=source;
      section.id=`quota-panel-${source}`;section.hidden=source!==selected;
      section.setAttribute('role','tabpanel');section.setAttribute('aria-labelledby',`quota-tab-${source}`);
      const scroll=el('div','quota-scroll');scroll.dataset.source=source;
      scroll.tabIndex=0;scroll.setAttribute('role','region');scroll.setAttribute('aria-label',`${name} 额度表，可横向滚动`);
      const table=el('table','client-matrix quota-table');table.setAttribute('aria-label',name);
      const fields=[['machine','机器'],...(grouped ? [['group','额度组']] : []),['window','窗口'],['remaining','已使用'],['api','API 价值'],['reset','重置倒计时'],
        ...(codex ? [['credits','Credits'],['cards','重置卡']] : []),['captured','采集']];
      const columns=el('colgroup'), head=el('thead'), labels=el('tr');
      for (const [key,label] of fields) {
        columns.append(el('col',`quota-col-${key}`));
        const cell=el('th',key==='credits'?'quota-credits':'',label);cell.scope='col';labels.append(cell);
      }
      head.append(labels);table.append(columns,head);
      for (const node of installedNodes) {
        const agent=UsageUI.accounts.get(accountKey(node,source)) || (node.data?.agents || []).find(a => a.source===source);
        const all=agent?.buckets || [];
        const credits=all.filter(b=>b.id==='credits' || b.id?.endsWith(':credits'));
        const cards=all.find(b=>b.id==='reset-credits');
        const buckets=codex ? all.filter(b=>!credits.includes(b) && b!==cards) : all;
        const rows=buckets.length ? buckets : [null];
        const body=el('tbody','quota-node');body.dataset.node=node.id;
        body.dataset.key=`node:${node.id}`;
        for (const [index, bucket] of rows.entries()) {
          const row=el('tr');
          row.dataset.key=`bucket:${bucket?.id ?? index}`;
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
          const api=el('td');cycles.paint(api,node,source,bucket);
          row.append(remaining,api,deadline(bucket?.resets_at));
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
                detail.textContent=expiry.textContent;detail.dataset.deadline=cards.expires_at;detail.dataset.expiry='true';detail.title=expiry.title;
                card.append(detail);
              }
              row.append(balance,card);
            }
            const captured=el('td','quota-time',time(agent?.captured_at));
            if (agent?.captured_at) captured.title=new Date(agent.captured_at).toLocaleString();
            captured.rowSpan=rows.length;
            const error=node.error || agent?.error;
            if (error) { const warning=el('div','quota-error',node.error || (agent.error_code==='cli_missing'?'CLI 文件缺失':agent.error_code==='timeout'?'查询超时':'额度查询失败'));warning.title=error;captured.append(warning); }
            row.append(captured);
          }
          body.append(row);
        }
        table.append(body);
      }
      scroll.append(table);section.append(scroll);next.append(section);
    }
    update(root,next);
    if(previous!==selected)viewport.scrollTop=positions.get(selected) || 0;
  }
  const accountKey=(node,source)=>`${hub ? node.id : ''}:${source}`;
  function accept(node,data) {
    node.data=data;node.error=null;
    for(const quota of data.agents || [])UsageUI.publishQuota(accountKey(node,quota.source),quota);
    render();
  }
  UsageUI.watchQuota(root,null,()=>render());
  async function load(collect=false) {
    refresh.disabled=true;status.textContent='正在查询额度…';
    try {
      if(!nodes.length) {
        const data=await request('api/nodes');hub=data.mode==='hub';
        nodes=data.nodes || [];
        cycles.start(nodes,hub);
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
          if(quotas.status==='fulfilled') {accept(node,quotas.value);}
          else if(collect) throw quotas.reason;
          else node.error=quotas.reason.message;
          render();
          // Each node refreshes after its own cached read, without waiting for peers.
          if(!collect && !stopped) {
            accept(node,await request(base+'/refresh?cached=true','POST'));
          }
        }
        catch(error) {failures++;node.error=error.message;render();}
      }));
      status.textContent=nodes.length ? (failures ? nodes.filter(n=>n.error).map(n=>`${n.name}：${n.error}`).join('；') : (root.children.length ? '' : 'N/A')) : 'N/A';
    } catch(error) {status.textContent=`读取失败 · ${error.message}`;}
    finally {refresh.disabled=false;}
  }
  refresh.addEventListener('click',async()=>{await load(true);await cycles.refresh();});
  // Only update the local clock; opening and the refresh button collect quotas.
  const timer=setInterval(tick,1000);
  const stopClock=()=>{stopped=true;cycles.stop();UsageUI.watchers.delete(root);clearInterval(timer);window.removeEventListener('pagehide',stopClock);};
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
    dialog.innerHTML='<div class="quota-panel"><div class="modal-head"><h2 id="quota-title">Agent 用量</h2><button id="quota-refresh" class="btn" type="button">刷新</button><button class="modal-close" type="button" aria-label="关闭">×</button></div><div id="quota-tabs" class="quota-tabs" role="tablist" aria-label="Agent 类型" hidden></div><div class="quota-body"><p id="quota-status" role="status"></p><div id="quota-agents"></div></div></div>';
    dialog.querySelector('.modal-close').onclick=()=>dialog.close();
    dialog.addEventListener('click',e=>{if(e.target===dialog) dialog.close();});
    dialog.addEventListener('close',()=>dialog.remove(),{once:true});
    document.body.append(dialog);dialog.showModal();
    initQuotas(dialog);
  });
})();
