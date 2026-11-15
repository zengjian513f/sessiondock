'use strict';

// The existing quota table owns the layout. This controller only supplies its
// API-value column from each node's independently captured accounting snapshot.
function initCycleUsage(changed) {
  let nodes=[],hub=false,stopped=false,timer=null,busy=false;
  const money=n=>Number.isFinite(n)?n.toLocaleString('en-US',{style:'currency',currency:'USD'}):'N/A';
  const range=(low,high)=>money(low)+(Number.isFinite(high) && high>low?`–${money(high)}`:'');
  const time=t=>t?new Date(t).toLocaleString():'N/A';
  async function load(collect=false) {
    if(stopped || busy)return;
    if(document.hidden && !collect){schedule(60000);return;}
    busy=true;
    await Promise.all(nodes.map(async node=>{
      try {
        const base=hub?`api/nodes/${encodeURIComponent(node.id)}/api/usage`:'api/usage';
        const response=await fetch(base+(collect?'/refresh':''),{method:collect?'POST':'GET',cache:'no-store'});
        if(!response.ok)throw new Error(`HTTP ${response.status}`);
        node.usage=await response.json();node.usageError=null;
      } catch(error){node.usageError=error.message;}
      if(!stopped)changed();
    }));
    busy=false;
    schedule(nodes.some(n=>n.usage?.running)?1000:60000);
  }
  function schedule(delay){clearTimeout(timer);if(!stopped)timer=setTimeout(()=>load(),delay);}
  return {
    start(list,isHub){nodes=list;hub=isHub;load();},
    refresh(){return load(true);},
    stop(){stopped=true;clearTimeout(timer);},
    paint(cell,node,source,bucket) {
      cell.className='quota-api';cell.tabIndex=0;
      const data=node.usage,row=data?.rows?.find(r=>r.source===source && r.id===bucket?.id);
      const sameCycle=row && Date.parse(row.end)===Date.parse(bucket.resets_at);
      if(!sameCycle){cell.textContent='N/A';cell.title=node.usageError || (data?.running?'正在统计…':'尚无本周期统计');return;}
      cell.textContent=(row.partial && row.api_usd!=null?'≥ ':'')+(row.partial?money(row.api_usd):range(row.api_usd,row.api_usd_max));
      const stale=node.usageError || data.error || row.end && Date.parse(row.end)<=Date.now() || row.quota_at && Date.now()-Date.parse(row.quota_at)>900000;
      const detail=[row.scope,`周期：${time(row.start)} → ${time(row.end)}`,`费用截至：${time(row.cost_until || row.quota_at)}`];
      if(row.exhausted_at)detail.push('已在首次显示 100% 时截止；后续 Credits 消费不计入。');
      if(row.reason)detail.push(row.reason);
      detail.push(...(row.notes || []));
      if(row.partial && row.api_usd!=null){const hint=document.createElement('small');hint.className='quota-api-note';hint.textContent='记录不完整';cell.append(hint);}
      if(stale)detail.push('快照过期或读取失败，暂不估算整轮价值。');
      if(row.equivalent_usd!=null && !stale){
        const estimate=document.createElement('small');estimate.textContent=`整轮 ≈ ${range(row.equivalent_usd,row.equivalent_usd_max)}`;cell.append(estimate);
        const point=row.estimate_point;
        detail.push(point?`首次显示 ${point.observed_percent}%（${time(point.at)}）：${money(point.api_usd)} ÷ ${point.percent}% ≈ ${money(row.equivalent_usd)}${point.rounded?'；半个百分点边界按四舍五入估计':''}`:
          `${range(row.api_usd,row.api_usd_max)} ÷ ${row.used_percent}% ≈ ${range(row.equivalent_usd,row.equivalent_usd_max)}`);
      }
      if(row.base_api_usd!=null && !row.api_usd_max)detail.push(`不含长上下文加价：${money(row.base_api_usd)}`);
      for(const part of row.models || [])detail.push(`${part.model || '未知型号'}：${range(part.api_usd,part.api_usd_max)}，${part.requests} 次调用${part.unpriced?'（含未定价）':''}`);
      detail.push(`Standard API 等值，非实际扣款；价目 ${data.price_version || 'N/A'}。`,data.running?'正在增量更新…':`统计更新：${time(data.captured_at)}`);
      cell.title=detail.filter(Boolean).join('\n');cell.setAttribute('aria-label',`API 价值 ${cell.textContent}。${cell.title}`);
    }
  };
}
