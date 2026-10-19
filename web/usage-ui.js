'use strict';

// Shared labels and quota identity for SDK and conversation status bars.
const UsageUI = {
  minutes(value) {
    if (typeof value === 'number') return value;
    const named = {five_hour:300, seven_day:10080, weekly:10080};
    if (Object.hasOwn(named,value)) return named[value];
    const match = /^(\d+(?:\.\d+)?)([dhm])$/i.exec(value || '');
    return match ? Number(match[1]) * ({d:1440,h:60,m:1})[match[2].toLowerCase()] : NaN;
  },
  window(value) {
    if(value==='spend_limit')return '消费限额';
    const minutes = this.minutes(value);
    if (!Number.isFinite(minutes) || minutes <= 0) return value || '额度';
    return minutes % 1440 === 0 ? `${minutes/1440}天` : minutes % 60 === 0 ? `${minutes/60}小时` : `${minutes}分钟`;
  },
  group(value) {
    return {'Gemini Models':'Gemini','Claude and GPT models':'Claude/GPT'}[value] || value || '额度';
  },
  effort(value) { return {none:'无',minimal:'最低',low:'低',medium:'中',high:'高',xhigh:'极高',max:'最高'}[value] || value; },
  percent(value) { return value.toLocaleString(undefined,{maximumFractionDigits:1}); },
  time(value) { return new Date(value).toLocaleString(undefined,{month:'2-digit',day:'2-digit',hour:'2-digit',minute:'2-digit',hour12:false}); },
  groups(buckets) {
    const groups = new Map();
    for (const bucket of buckets || []) {
      const name = bucket.group || bucket.id || '额度';
      if (!groups.has(name)) groups.set(name,[]);
      groups.get(name).push(bucket);
    }
    return [...groups].map(([name,rows])=>[name,rows.sort((a,b)=>(this.minutes(a.window)||Infinity)-(this.minutes(b.window)||Infinity))]);
  },
  gauge(label,used,title) {
    const node=el('span','sdk-gauge');node.title=title;
    const value=Math.max(0,Math.min(100,used)),bar=el('i','sdk-bar'),fill=el('u','');
    fill.style.width=`${value}%`;bar.append(fill);
    for(const [key,text] of Object.entries({role:'progressbar','aria-label':`${label} 已用`,'aria-valuemin':'0','aria-valuemax':'100','aria-valuenow':String(value)}))bar.setAttribute(key,text);
    node.append(el('span','',label),bar,el('b','',`${Math.round(value)}%`));node.classList.toggle('low',Math.round(value)>=80);
    return node;
  },
  appendQuotas(holder,quota,source) {
    const groups=this.groups(quota?.buckets).filter(([,rows])=>rows.some(bucket=>Number.isFinite(bucket.remaining_percent)));
    for(const [name,buckets] of groups) {
      let target=holder;
      if(source==='agy' || groups.length>1) {
        const group=el('span','sdk-quota-group'),label=el('span','sdk-quota-name',this.group(name));label.title=name;
        target=el('span','sdk-quota-windows');group.append(label,target);holder.append(group);
      }
      for(const bucket of buckets) {
        if(!Number.isFinite(bucket.remaining_percent))continue;
        const label=this.window(bucket.window || (source==='agy'?'额度':name)),used=100-bucket.remaining_percent;
        const title=`${name} · ${label} 已用 ${this.percent(used)}%，剩余 ${this.percent(bucket.remaining_percent)}%${bucket.resets_at ? `；${this.time(bucket.resets_at)} 重置`:''}${quota.captured_at ? `；更新于 ${this.time(quota.captured_at)}`:''}`;
        const gauge=this.gauge(label,used,title);
        gauge.querySelector('[role=progressbar]').setAttribute('aria-label',`${name} ${label} 已用`);
        target.append(gauge);
      }
      if(target!==holder && !target.childElementCount)target.parentElement.remove();
    }
  },
};
