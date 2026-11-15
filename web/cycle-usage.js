'use strict';

// Cycle money and official quota percentages are one snapshot. Do not combine
// a newer quota broadcast with an older numerator in the browser.
function initCycleUsage(container) {
  const root = container.querySelector('#usage-cycles');
  if (!root) return {start(){}, render(){}, stop(){}};
  const names = {codex:'Codex',claude:'Claude Code',grok:'Grok',agy:'Agy',kiro:'Kiro',droid:'Droid',opencode:'OpenCode',codebuddy:'CodeBuddy'};
  let nodes=[], hub=false, stopped=false, timer=null, busy=false, activeSettings=null;
  const state=new Map();
  const el=(tag,cls,text)=>{const n=document.createElement(tag);n.className=cls || '';if(text!=null)n.textContent=text;return n;};
  const money=value=>!Number.isFinite(value)?'—':value>0 && value<.01?'<$0.01':value.toLocaleString('en-US',{style:'currency',currency:'USD'});
  const time=value=>value ? new Date(value).toLocaleString(undefined,{month:'2-digit',day:'2-digit',hour:'2-digit',minute:'2-digit'}) : '—';
  const base=node=>hub?`api/nodes/${encodeURIComponent(node.id)}/api/usage`:'api/usage';
  async function request(node,suffix='',body) {
    const response=await fetch(base(node)+suffix,{method:suffix?'POST':'GET',cache:'no-store',
      ...(body?{headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}:{})});
    const result=await response.json();
    if(!response.ok)throw new Error(result.error || `HTTP ${response.status}`);
    return result;
  }
  const toolbar=el('div','usage-toolbar'), heading=el('h2','','本周期用量'),refresh=el('button','btn','更新用量');
  refresh.type='button';refresh.id='usage-refresh';toolbar.append(heading,refresh);
  const note=el('p','usage-note','预计套餐等值 = 同周期 API 等价消耗 ÷ 已用比例。5小时、7天与月度分别估算；各机器不相加。');
  const status=el('p','usage-note');status.setAttribute('role','status');status.id='usage-status';
  const content=el('div'),settingsArea=el('div');
  root.append(toolbar,note,status,content,settingsArea);

  function detail(row,data) {
    const box=el('details','usage-details'),summary=el('summary','','明细');box.append(summary);
    box.append(el('p','usage-note',`${row.scope}。${data.basis || ''}。`));
    box.append(el('p','usage-note',`周期：${time(row.start)} → ${time(row.end)}；费用截至官方额度采集时间 ${time(row.quota_at)}；统计更新 ${time(row.captured_at)}`));
    if(row.equivalent_usd!=null)box.append(el('p','usage-note',`${money(row.api_usd)} ÷ (${row.used_percent.toLocaleString(undefined,{maximumFractionDigits:6})}% ÷ 100) ≈ ${money(row.equivalent_usd)}。按当前模型组合估算，不是保证可用的金额。`));
    for(const part of row.models || []) {
      const t=part.tokens || {};
      box.append(el('p','usage-model',`${part.model || '未知模型'} · ${money(part.api_usd)}${part.unpriced?'（部分）':''} · ${part.requests} 条请求`));
      box.append(el('p','usage-note',`输入 ${(t.input || 0).toLocaleString()} · 输出 ${(t.output || 0).toLocaleString()} · 缓存读取 ${(t.cache_read || 0).toLocaleString()} · 缓存写入 ${((t.cache_write || 0)+(t.cache_write_long || 0)).toLocaleString()} Token`));
    }
    box.append(el('p','usage-note',`价目版本：${data.price_version || '未返回'}；缺少价格的用量不会按零计费。`));
    for(const url of data.price_sources || []) {
      if(!/^https:\/\//.test(url))continue;
      const a=el('a','usage-price-link',new URL(url).hostname);a.href=url;a.target='_blank';a.rel='noopener noreferrer';box.append(a);
    }
    return box;
  }

  function render() {
    const open=new Set([...content.querySelectorAll('details[open]')].map(n=>n.dataset.key));
    content.replaceChildren();
    let running=false;
    for(const node of nodes) {
      const data=state.get(node.id);
      const block=el('section','usage-node');block.dataset.node=node.id;
      const head=el('div','usage-toolbar'),title=el('h3','',node.name || '本机'),configure=el('button','btn','统计设置');
      configure.type='button';configure.setAttribute('aria-label',`${node.name || '本机'} 统计设置`);
      configure.disabled=!data?.configured;configure.onclick=()=>showSettings(node,data);
      head.append(title,configure);block.append(head);
      if(!data)block.append(el('p','usage-note','正在读取统计快照…'));
      else {
        running ||= !!data.running;
        if(data.error)block.append(el('p','quota-error',data.error));
        const info=data.running?'正在增量更新…':data.captured_at?`更新于 ${time(data.captured_at)} · ${data.settings?.enabled===false?'自动更新已关闭':`每 ${(data.settings?.interval_seconds || 300)/60} 分钟更新`}`:'尚未采集';
        block.append(el('p','usage-note',info));
        const rows=(data.rows || []).filter(row=>!node.clients || node.clients.some(c=>c.installed && c.source===row.source));
        if(!rows.length)block.append(el('p','usage-note',data.configured?'尚无可用统计，点击“更新用量”开始采集。':'此节点未配置用量统计。'));
        for(const row of rows) {
          const item=el('article','usage-cycle');item.dataset.source=row.source;item.dataset.window=row.id;
          const caption=el('div','usage-cycle-name');
          const group=row.group && row.group.toLowerCase()!==row.source ? UsageUI.group(row.group)+' · ' : '';
          caption.append(el('b','',names[row.source] || row.source),el('span','usage-note',`${group}${row.window==='calendar'?'本月（UTC）':UsageUI.window(row.window)}`));
          const metrics=el('dl','usage-metrics');
          const stale=row.window!=='calendar' && (row.end && Date.parse(row.end)<=Date.now() || row.quota_at && Date.now()-Date.parse(row.quota_at)>900000) || data.error;
          const equivalent=stale?null:row.equivalent_usd;
          for(const [label,value] of [['本周期 API 等价',`${row.partial && row.api_usd!=null?'≥ ':''}${money(row.api_usd)}`],
            ['已耗费',Number.isFinite(row.used_percent)?`${row.used_percent.toLocaleString(undefined,{maximumFractionDigits:2})}%`:'—'],
            ['预计套餐等值',equivalent==null?'—':`≈ ${money(equivalent)}`]]) {
            const pair=el('div');pair.append(el('dt','',label),el('dd','',value));metrics.append(pair);
          }
          item.append(caption,metrics);
          const reason=stale?'额度快照已过期或节点读取失败，请更新后再估算':row.reason;
          item.append(el('p','usage-note',reason || (row.local_sample?'本机样本估值；同账号在其他设备的消费会使此值偏低。':'按本机记录与当前模型组合估算整个额度窗口。')));
          const details=detail({...row,equivalent_usd:equivalent},data);details.dataset.key=`${node.id}:${row.source}:${row.id}`;details.open=open.has(details.dataset.key);item.append(details);block.append(item);
        }
      }
      content.append(block);
    }
    refresh.disabled=busy || running;
  }

  function showSettings(node,data) {
    activeSettings=node.id;settingsArea.replaceChildren();
    const form=el('form','usage-settings');form.setAttribute('aria-label','统计设置');
    form.append(el('h3','',`${node.name || '本机'} · 统计设置`));
    const enabled=el('input');enabled.type='checkbox';enabled.checked=data.settings?.enabled!==false;
    const label=el('label','usage-setting');label.append(enabled,document.createTextNode('周期性更新'));form.append(label);
    const interval=el('input');interval.type='number';interval.min='1';interval.step='1';interval.required=true;interval.value=data.settings?.interval_seconds || 300;interval.setAttribute('aria-label','更新间隔（秒）');
    const field=el('label','usage-setting','更新间隔（秒）');field.append(interval);form.append(field);
    const overrides=[];
    for(const row of (data.rows || []).filter(r=>r.window==='monthly')) {
      const label=el('label','usage-setting',`${names[row.source]} ${row.group || ''} 本周期起点`),input=el('input');input.type='datetime-local';
      input.setAttribute('aria-label',label.textContent);
      const override=data.settings?.sources?.[row.source]?.windows?.[row.id];
      const saved=override?.reset===row.end ? override.start : null;
      if(saved){const d=new Date(saved);input.value=new Date(d-d.getTimezoneOffset()*60000).toISOString().slice(0,16);}
      label.append(input);form.append(label);overrides.push({row,input});
    }
    form.append(el('p','usage-note','月度窗口若缺少官方起点，请按账单设置本周期起点（本地时区）。固定窗口自动按重置时间计算。'));
    const message=el('p','usage-note');message.setAttribute('role','status');
    const actions=el('div','usage-toolbar'),save=el('button','btn','保存'),cancel=el('button','btn','取消');save.type='submit';cancel.type='button';
    cancel.onclick=()=>{activeSettings=null;settingsArea.replaceChildren();};actions.append(save,cancel);form.append(message,actions);
    form.onsubmit=async e=>{
      e.preventDefault();save.disabled=true;
      const settings=structuredClone(data.settings || {});settings.enabled=enabled.checked;settings.interval_seconds=Number(interval.value);settings.sources ||= {};
      for(const {row,input} of overrides){const source=settings.sources[row.source] ||= {};source.windows ||= {};if(input.value)source.windows[row.id]={start:new Date(input.value).toISOString(),reset:row.end};else delete source.windows[row.id];}
      try {state.set(node.id,await request(node,'/settings',settings));activeSettings=null;settingsArea.replaceChildren();render();schedule(1000);}
      catch(error){message.textContent=`保存失败：${error.message}`;save.disabled=false;}
    };
    settingsArea.append(form);form.scrollIntoView({block:'nearest'});enabled.focus();
  }

  function schedule(delay) {clearTimeout(timer);if(!stopped)timer=setTimeout(()=>load(false),delay);}
  async function load(collect) {
    if(stopped || busy)return;
    if(document.hidden && !collect){schedule(60000);return;}
    busy=true;refresh.disabled=true;
    await Promise.all(nodes.map(async node=>{
      try {state.set(node.id,await request(node,collect?'/refresh':''));}
      catch(error){state.set(node.id,{...state.get(node.id),running:false,error:`读取失败：${error.message}`});}
      if(!stopped)render();
    }));
    busy=false;
    if(stopped)return;
    render();status.textContent=[...state.values()].some(d=>d.running)?'后台正在统计，关闭页面后仍会继续。':'';
    schedule([...state.values()].some(d=>d.running)?1000:60000);
  }
  refresh.onclick=()=>load(true);
  return {
    start(list,isHub){if(nodes.length)return;nodes=list;hub=isHub;load(false);},
    render,
    stop(){stopped=true;clearTimeout(timer);activeSettings=null;}
  };
}
