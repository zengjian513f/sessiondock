'use strict';

// Shared labels and quota identity for SDK and conversation status bars.
const UsageUI = {
  sessions: new Map(),
  accounts: new Map(),
  accountKey(uid) { return `${HUB_MODE ? nodeOf(uid) : ''}:${String(uid).split(':')[0]}`; },
  remembered(uid, agent) { return this.sessions.get(viewKey(uid,agent)); },
  remember(uid, agent, data) {
    if (!uid) return;
    const key = viewKey(uid, agent);
    this.sessions.delete(key);
    if (this.sessions.size >= 500) this.sessions.delete(this.sessions.keys().next().value);
    this.sessions.set(key, structuredClone(data));
  },
  quota(uid) { return this.accounts.get(this.accountKey(uid)); },
  rememberQuota(uid, quota) { if(quota)this.accounts.set(this.accountKey(uid),quota); },
  async readQuota(uid, signal) {
    const node=HUB_MODE ? nodeOf(uid) : '';
    const path=node ? `api/nodes/${encodeURIComponent(node)}/api/quotas` : 'api/quotas';
    const response=await fetch(appUrl(path),{cache:'no-store',signal});
    if(!response.ok)throw new Error('quota snapshot unavailable');
    const data=await response.json();
    return data.agents?.find(item=>item.source===String(uid).split(':')[0]);
  },
  minutes(value) {
    if (typeof value === 'number') return value;
    const named = {five_hour:300, seven_day:10080, weekly:10080};
    if (Object.hasOwn(named,value)) return named[value];
    const match = /^(\d+(?:\.\d+)?)([dhm])$/i.exec(value || '');
    return match ? Number(match[1]) * ({d:1440,h:60,m:1})[match[2].toLowerCase()] : NaN;
  },
  window(value) {
    if(value==='spend_limit')return '消费限额';
    if(value==='monthly')return '月度';
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
  countdown(value) {
    const end = typeof value === 'number' ? (value < 1e11 ? value * 1000 : value) : Date.parse(value);
    if (!Number.isFinite(end)) return 'N/A';
    const remaining = end - Date.now();
    if (remaining <= 0) return '已到期';
    const minutes = Math.floor(remaining / 60000);
    const days = Math.floor(minutes / 1440);
    return (days ? `${days}d ` : '') + String(Math.floor(minutes / 60) % 24).padStart(2,'0')
      + ':' + String(minutes % 60).padStart(2,'0');
  },
  reset(value) { return value == null || value === '' ? '' : `；重置剩余 ${this.countdown(value)}`; },
  clocks: new Map(),
  clock: null,
  title(node, value, description=false) {
    this.clocks.delete(node);
    const update = () => {
      const text = typeof value === 'function' ? value() : value;
      if (node.title !== text) node.title = text;
      if (description && node.getAttribute('aria-description') !== text) node.setAttribute('aria-description',text);
    };
    update();
    if (typeof value !== 'function') return;
    this.clocks.set(node,update);
    if (this.clock == null) this.clock = setInterval(() => {
      for (const [element,refresh] of this.clocks) {
        if (element.isConnected) refresh(); else this.clocks.delete(element);
      }
      if (!this.clocks.size) { clearInterval(this.clock); this.clock = null; }
    },1000);
  },
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
    const node=el('span','sdk-gauge');this.title(node,title);
    const value=Math.max(0,Math.min(100,used)),bar=el('i','sdk-bar'),fill=el('u','');
    fill.style.width=`${value}%`;bar.append(fill);
    for(const [key,text] of Object.entries({role:'progressbar','aria-label':`${label} 已用`,'aria-valuemin':'0','aria-valuemax':'100','aria-valuenow':String(value)}))bar.setAttribute(key,text);
    const display=value>0 && value<0.1 ? '<0.1%' : value<100 && value>99.9 ? '>99.9%' : `${this.percent(value)}%`;
    node.append(el('span','',label),bar,el('b','',display));node.classList.toggle('low',value>=80);
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
        const title=()=>`${name} · ${label} 已用 ${this.percent(used)}%，剩余 ${this.percent(bucket.remaining_percent)}%${this.reset(bucket.resets_at)}${quota.captured_at ? `；更新于 ${this.time(quota.captured_at)}`:''}`;
        const gauge=this.gauge(label,used,title);
        gauge.querySelector('[role=progressbar]').setAttribute('aria-label',`${name} ${label} 已用`);
        target.append(gauge);
      }
      if(target!==holder && !target.childElementCount)target.parentElement.remove();
    }
  },
};
