'use strict';

// Native capabilities share presentation, never another provider's wire semantics.
const NativeSdkControls = {
  labels: Object.freeze({claude:'Claude',codex:'Codex',grok:'Grok',opencode:'OpenCode',agy:'Agy',kiro:'Kiro',codebuddy:'CodeBuddy',droid:'Droid'}),
  badgeClass(source) { return `b-${source || 'unknown'}`; },
  formatReset(value) {
    if (value == null || value === '') return '';
    if (typeof value === 'number' && Number.isFinite(value)) {
      return UsageUI.reset(value > 1e11 ? Math.round(value / 1000) : value);
    }
    const str = String(value).trim();
    if (!str) return '';
    if (/^\d+(\.\d+)?$/.test(str)) {
      const n = Number(str);
      return UsageUI.reset(n > 1e11 ? Math.round(n / 1000) : n);
    }
    const parsed = Date.parse(str);
    if (Number.isFinite(parsed)) return UsageUI.reset(Math.round(parsed / 1000));
    return str.startsWith('·') ? ` ${str}` : ` · ${str}${str.includes('重置') ? '' : ' 后重置'}`;
  },
  timerDetails(timers, receipt, runtimeError, agents = [], uid) {
    return {
      layout: 'timers',
      key: JSON.stringify([timers, receipt, runtimeError, agents, uid]),
      render: menu => this.paintTimerDetails(menu, timers, receipt, runtimeError, agents, uid),
    };
  },
  timerDelay(delay) {
    const units = {ms: 1, s: 1000, m: 60000, h: 3600000, d: 86400000};
    const input = delay.replace(/\s+/g, '');
    const parts = [...input.matchAll(/(\d+(?:\.\d+)?)(ms|s|m|h|d)/g)];
    if (!input || parts.map(part => part[0]).join('') !== input) return NaN;
    return parts.reduce((total, [, amount, unit]) => total + Number(amount) * units[unit], 0);
  },
  scheduledTimers(timers) {
    const deadline = timer => Number.isFinite(timer.due) ? timer.due : Number.MAX_VALUE;
    return timers.map(timer => {
      const delay = this.timerDelay(timer.delay || '');
      const created = Date.parse(timer.created_at);
      const due = timer.due_at ? Date.parse(timer.due_at) : new Date(created + delay).getTime();
      return {...timer, created, due, delayMs: delay};
    }).sort((left, right) => deadline(left) - deadline(right));
  },
  pendingTimers(timers) {
    const now = Date.now();
    return this.scheduledTimers(timers).filter(timer => !Number.isFinite(timer.due) || timer.due > now);
  },
  paintTimerDetails(menu, timers, receipt, runtimeError, agents, uid) {
    const list = el('ul', 'sdk-timer-list');
    const header = el('div', 'sdk-control-section sdk-timer-heading');
    header.append(el('h3', '', timers.length ? '后台定时任务' : '子代理有后台任务'), el('p', 'sdk-control-note', receipt));
    if (runtimeError) header.append(el('p', 'sdk-control-note', runtimeError));

    for (const timer of timers) list.append(this.timerItem(timer));
    const scrollTop = menu.querySelector('.sdk-timer-body')?.scrollTop || 0;
    const body = el('div', 'sdk-timer-body');
    if (timers.length) body.append(list);
    if (agents.length) {
      const links = el('div', 'sdk-task-agents');
      if (timers.length) links.append(el('h4', '', '子代理有后台任务'));
      for (const agent of agents) {
        const button = el('button', 'btn');
        button.type = 'button';
        const namesakes = agents.filter(other => other.id !== agent.id && other.title === agent.title);
        const shortId = agent.id.slice(0, 8);
        const id = namesakes.some(other => other.id.startsWith(shortId)) ? agent.id : shortId;
        button.textContent = agent.title && !namesakes.length ? agent.title : [agent.title, id].filter(Boolean).join(' · ');
        button.title = '查看子代理 · ' + agent.id;
        button.append(el('span', '', '›'));
        button.lastChild.setAttribute('aria-hidden', 'true');
        button.onclick = () => { menu.hidePopover(); openSession(uid, agent.id); };
        links.append(button);
      }
      body.append(links);
    }
    menu.replaceChildren(header, body);
    body.scrollTop = scrollTop;
  },
  timerItem(timer, showTask = false) {
    const time = (stamp, compact = false) => {
      const formatted = ConversationUI.messageTimeFormatter.format(stamp);
      const node = el('time', '', compact ? formatted.slice(5, 16) : formatted);
      node.dateTime = new Date(stamp).toISOString();
      node.title = formatted;
      return node;
    };
    const item = el('li', 'sdk-timer-item');
    if (timer.prompt) {
      const prompt = el('p', 'sdk-timer-prompt');
      prompt.textContent = timer.prompt;
      item.append(prompt);
    }
    const timing = el('div', 'sdk-timer-timing');
    if (Number.isFinite(timer.due)) {
      const due = el('span', 'sdk-timer-due', '预计触发 ');
      due.append(time(timer.due, true));
      timing.append(due);
    }
    const delay = Number.isFinite(timer.delayMs) ? formatDuration(timer.delayMs) : timer.delay;
    if (delay) {
      const span = el('span');
      span.textContent = '延时 ' + delay;
      timing.append(span);
    }
    if (timer.schedule) timing.append(el('span', '', timer.schedule));
    if (timing.childElementCount) item.append(timing);
    const origin = el('div', 'sdk-timer-origin');
    if (showTask && timer.task) origin.textContent = TaskOutput.number(timer.task);
    if (Number.isFinite(timer.created)) origin.append(origin.textContent ? ' · 设置于 ' : '设置于 ', time(timer.created));
    if (origin.textContent) item.append(origin);
    return item;
  },
  mount(view) {
    view.settingChoices = new Map(); view.nativeSettingValues = new Map();
    view.usageRead=null;view.usageRetryTimer=0;view.usageRetryDelay=1000;
    view.usageData=UsageUI.remembered(view.uid,view.agent) || {};
    view.usageData.quota=UsageUI.quota(view.uid) || view.usageData.quota;
    view.quotaRevision=0;
    UsageUI.watchQuota(view.pane,UsageUI.accountKey(view.uid),quota=>{
      if(view.usageData.quota===quota)return;
      view.usageData.quota=quota;view.quotaRevision++;this.paintUsage(view);
    });
    view.nativeSettings=[];view.nativeActionPending=false;
    const menu = view.pane.querySelector('#sdk-more-menu');
    menu.classList.add('sdk-native-menu');
    // Other model pools share ⋯ with controls that do not fit.
    const actions = el('div',''); actions.id = 'sdk-actions'; menu.append(actions);
    const source = view.uid.split(':')[0], composerSteer = ['kiro','droid','agy'].includes(source);
    const actionDefs = [
      ['steer','引导当前任务',composerSteer
        ? '引导当前任务：将输入框内容立即插队发给运行中回合（Ctrl+Enter；空闲时直接作为新回合发送）'
        : '引导当前任务：向正在运行的回合插队发送补充指令'],
      ['compact','压缩上下文',source==='droid'
        ? '压缩上下文：压缩早期对话历史以释放上下文空间；若原生生成新会话将自动切换（/compact）'
        : '压缩上下文：压缩早期对话历史以释放上下文空间（/compact）'],
      ['unrevert','撤销回退','撤销回退：取消当前暂存的对话回退，恢复被隐藏的消息'],
      ['revert_commit','提交回退','提交回退：永久移除已暂存回退的对话记录，不修改工作区文件'],
      ['shell','运行命令','运行命令：在会话工作目录直接执行 Shell 命令，不经过模型推理（等同输入 !命令）'],
      ['command','原生命令','原生命令：选择并执行当前原生服务注册的斜杠命令'],
      ['permissions','权限规则','权限规则：查看或编辑当前会话的 OpenCode 原生权限规则 JSON'],
    ];
    for (const [method,label,tooltip] of actionDefs) {
      const button = el('button','new-model-option sdk-action'); button.type = 'button';button.dataset.sdkAction = method;button.title=tooltip;button.setAttribute('aria-label',label);
      button.innerHTML=uiIcon(({compact:'compact',unrevert:'undo',revert_commit:'continue',steer:'continue',shell:'terminal',command:'list',permissions:'settings'})[method]);button.append(el('span','sdk-action-label',label));
      button.onclick = () => { view.closeToolbarMenus(); void this.action(view,method); }; actions.append(button);
    }
    const settings = el('div','sdk-control-section'); settings.id = 'native-sdk-settings'; menu.append(settings);
    this.paint(view);
    // Account snapshots are independent of native startup; never wait for /open.
    if(['agy','claude','kiro','droid'].includes(view.uid.split(':')[0])) {
      const generation=view.generation,revision=view.quotaRevision,read=new AbortController();view.quotaRead=read;
      void UsageUI.readQuota(view.uid,read.signal).then(quota=>{
        if(!view.current(generation) || revision!==view.quotaRevision || !quota)return;
        view.usageData.quota=quota;this.paintUsage(view);
      }).catch(()=>{});
    }
  },
  close(view) {
    UsageUI.watchers.delete(view.pane);
    if(view.uid)this.rememberUsage(view);
    view.quotaRead?.abort();view.quotaRead=null;
    clearTimeout(view.usageRetryTimer);view.usageRetryTimer=0;
    view.usageRead?.abort();view.usageRead=null;
    CodexSdkActions.close();
  },
  layout(view) {
    if (!view.pane) return;
    const gauges=view.pane.querySelector('.sdk-gauges'),more=view.pane.querySelector('#sdk-more-menu');
    if (!gauges || !more) return;
    SdkUI.layoutFooter(view, ['.sdk-model', '.sdk-effort'].map(selector => view.pane.querySelector(selector))
      .concat(view.nativeSettings || [],[...view.pane.querySelectorAll('[data-sdk-action]')]));
    if (more.matches(':popover-open')) ConversationUI.placePopover(more,view.pane.querySelector('#sdk-more'));
    view.updateStatusDisclosure();
  },
  async loadUsage(view,refresh) {
    const generation=view.generation;
    if (!view.uid || view.loadingUsage) return;
    clearTimeout(view.usageRetryTimer);view.usageRetryTimer=0;
    const read=new AbortController();view.usageRead=read;
    view.loadingUsage=true;
    try {
      let response=await view.request('api/native-sdk/usage',{uid:view.uid,refresh:refresh === 'auto' ? false : refresh},read.signal);
      if (!view.current(generation)) return;
      view.usageData={billing:view.usageData?.billing,quota:view.usageData?.quota,...response,...response.raw};view.usageError=null;
      if(response.quota || response.raw?.quota)view.quotaRevision++;
      this.paintUsage(view);
      if (refresh === 'auto') {
        response=await view.request('api/native-sdk/usage',{uid:view.uid,refresh},read.signal);
        if (!view.current(generation)) return;
        view.usageData={billing:view.usageData?.billing,quota:view.usageData?.quota,...response,...response.raw};
        if(response.quota || response.raw?.quota)view.quotaRevision++;
        this.paintUsage(view);
      }
      if (view.capabilities.billing) {
        try { const billing=await view.request('api/native-sdk/billing',{uid:view.uid},read.signal); if (view.current(generation)) view.usageData.billing=billing.raw?.billing || billing.raw; }
        catch (error) { if (view.current(generation)) view.usageError=`额度：${error.message}`; }
      }
    } catch (error) { if (view.current(generation)) view.usageError=error.message; }
    finally { if (view.current(generation)) {
      view.usageRead=null;view.loadingUsage=false;this.paintUsage(view);
      if(view.usageError){
        view.usageRetryTimer=setTimeout(()=>{if(view.current(generation))void this.loadUsage(view,refresh);},view.usageRetryDelay);
        view.usageRetryDelay=Math.min(view.usageRetryDelay*2,30000);
      }else view.usageRetryDelay=1000;
    } }
  },
  paint(view) {
    if (!view.pane) return;
    for (const button of view.pane.querySelectorAll('[data-sdk-action]')) {
      button.hidden=!this.supportsAction(view,button.dataset.sdkAction);
      if(view.uid.startsWith('droid:'))button.disabled=!!view.nativeActionPending;
    }
    this.paintSettings(view);this.paintUsage(view);this.paintQueue(view);this.layout(view);
  },
  hasStagedRevert(view) {
    const revert = view.raw?.revert;
    return Boolean(typeof revert === 'string' ? revert : revert?.messageID || revert?.message_id);
  },
  supportsAction(view,method) {
    // Permission requests do not imply OpenCode's permission-rule settings API.
    if(method==='permissions')return view.uid.startsWith('opencode:') && (view.capabilities.permissions===true || view.capabilities.settings===true);
    if(method==='unrevert' || method==='revert_commit')return view.capabilities[method]===true && this.hasStagedRevert(view);
    return view.capabilities[method]===true;
  },
  rememberUsage(view) {
    const data={...view.usageData};
    for(const key of ['usage','context_usage','billing','rate_limits','step_usage','usage_errors','credits','credit_limit','resets_at'])
      if(view.raw?.[key]!=null && (typeof view.raw[key]!=='object' || Object.keys(view.raw[key]).length))data[key]=view.raw[key];
    UsageUI.remember(view.uid,view.agent,data);
    UsageUI.rememberQuota(view.uid,data.quota);
  },
  paintUsage(view) {
    if (!view.pane) return;
    const gauges=view.pane.querySelector('.sdk-gauges');if(!gauges)return;
    gauges.replaceChildren();
    const usage=Object.keys(view.raw?.usage || {}).length ? view.raw.usage : view.usageData?.usage || {};
    const number=(...values)=>values.find(value=>typeof value==='number' && Number.isFinite(value));
    const count=value=>value.toLocaleString(undefined,{maximumFractionDigits:0});
    const creditCount=value=>value.toLocaleString(undefined,{maximumFractionDigits:2});
    const lines=[];
    const text=(label,value)=>{lines.push(()=>`${label}：${typeof value==='function' ? value() : value}`);};
    const gauge=(label,used,title)=>{
      if (!Number.isFinite(used)) return;
      gauges.append(UsageUI.gauge(label,used,title));text(label,title);
    };
    const context=usage.context || view.raw?.context_usage || view.usageData?.context_usage || {};
    const percent=number(context.percent,context.percentage);if(percent!=null)gauge('上下文',percent,`已用 ${percent.toFixed(1)}%${Number.isFinite(context.used_tokens) && Number.isFinite(context.max_tokens) ? `（${count(context.used_tokens)} / ${count(context.max_tokens)} tokens）`:''}；${context.basis || '原生估计'}`);
    UsageUI.rememberQuota(view.uid,view.usageData?.quota);
    const quota=UsageUI.quota(view.uid) || view.usageData?.quota;
    view.usageData.quota=quota;
    const source=view.uid.split(':')[0];
    const model=sdkSettingValue(view,'model',view.nativeModel || view.historyData?.model || '');
    const selection=UsageUI.quotaSelection(quota,source,model);
    UsageUI.appendQuotas(gauges,quota,source,selection.inline);
    UsageUI.paintQuotaMenu(view.pane.querySelector('#sdk-more-menu'),quota,selection.other);
    for(const bucket of quota?.buckets || []){
      const unit=bucket.unit || (view.uid.startsWith('kiro:') ? 'Credits' : '');
      const usedLimit=bucket.used!=null && bucket.limit!=null ? `${creditCount(bucket.used)} / ${creditCount(bucket.limit)}${unit ? ` ${unit}` : ''} · ` : '';
      text(`${bucket.group || bucket.id || 'Credit'} · ${UsageUI.window(bucket.window || unit || '额度')}`,()=>`${usedLimit}剩余 ${Number.isFinite(bucket.remaining_percent)?UsageUI.percent(bucket.remaining_percent)+'%':'未知'}${this.formatReset(bucket.resets_at ?? bucket.reset_at ?? bucket.reset)}`);
    }
    if(quota?.error)text('额度查询',quota.error);
    const billing=view.raw?.billing || view.usageData?.billing;const b=billing?.config || billing;
    if(b){gauge('额度',b.creditUsagePercent,()=>`本周期已用 ${b.creditUsagePercent}%${this.formatReset(b.currentPeriod?.end)}`);if(b.prepaidBalance?.val!=null)text('预付余额',`$${(b.prepaidBalance.val/100).toFixed(2)}`);}
    const rate=view.raw?.rate_limits || view.usageData?.rate_limits;
    for(const [window,limit] of Object.entries(rate || {})) {
      const used=number(limit?.utilization,limit?.used_percentage,limit?.usedPercent);
      if(used!=null)text(UsageUI.window(window),()=>`已用 ${UsageUI.percent(used)}%${this.formatReset(limit.resets_at)}`);
    }
    for(const [key,error] of Object.entries(view.raw?.usage_errors || {}))text(`用量 ${key}`,error);
    if(view.raw?.settings?.fast_mode_state)text('Fast 状态',JSON.stringify(view.raw.settings.fast_mode_state));
    if(view.raw?.settings?.fast_mode_disabled_reason)text('Fast 不可用原因',view.raw.settings.fast_mode_disabled_reason);
    if(view.usageError)text('查询未完成',view.usageError);
    if(!lines.length)text('用量','原生尚未提供用量数据');
    UsageUI.title(gauges,()=>lines.map(line=>line()).join(''),true);
    this.rememberUsage(view);
    this.layout(view);
  },
  paintSettings(view) {
    const holder=view.pane.querySelector('#native-sdk-settings'),raw=view.raw || {},source=view.uid.split(':')[0];
    const modes=raw.modes?.availableModes || (Array.isArray(raw.modes)?raw.modes:null) || (source==='droid' && view.capabilities.mode ? ['auto','spec'] : source==='kiro' && (view.capabilities.mode || view.capabilities.settings) ? ['default','plan','auto'] : []);
    const definitions=[];
    if(view.capabilities.mode || (view.capabilities.settings && Array.isArray(modes) && modes.length))definitions.push(['mode','模式',Array.isArray(modes)?modes:[],raw.mode || raw.modes?.currentModeId || raw.settings?.mode || (source==='droid'?raw.settings?.interactionMode:null)]);
    if(source==='droid' && view.capabilities.permission_mode)definitions.push(['autonomy_level','自主权限',['off','low','medium','high'],raw.settings?.autonomy_level || raw.settings?.autonomyLevel]);
    if(source!=='droid' && (view.capabilities.permission_mode || view.capabilities.approval || (source==='kiro' && (view.capabilities.permissions || view.capabilities.settings))))definitions.push(['permission_mode','权限模式',raw.permission_modes || raw.approvals || (source==='kiro'?['default','autopilot']:['default','acceptEdits','plan','dontAsk','auto','bypassPermissions']),raw.permission_mode || raw.settings?.permission_mode || (typeof raw.autopilot==='boolean'?(raw.autopilot?'autopilot':'default'):raw.approval)]);
    if(view.capabilities.fast_mode)definitions.push(['fast_mode','Fast',['false','true'],raw.settings?.fast_mode==null?'':String(raw.settings.fast_mode)]);
    if(view.capabilities.settings && source==='opencode')definitions.push(['native_agent','代理',view.nativeAgents || raw.agents || [],raw.agent]);
    for(const definition of definitions){definition[3]=sdkSettingValue(view,definition[0],definition[3] || '');definition[4]=view.settingChoices.has(definition[0]);}
    const signature=JSON.stringify(definitions);if(holder.dataset.state===signature){sdkSyncSettings(view);return;}holder.dataset.state=signature;
    view.pane.querySelector('.sdk-status-row').dataset.footerLayout='';
    const existing=new Map((view.nativeSettings || []).map(label=>[label.querySelector('select')?.id,label]));
    const nextSettings=[];
    for(const [key,title,options,current,pending] of definitions) {
      const selectId=`native-sdk-${key}`;
      let label=existing.get(selectId),select=label?.querySelector('select');
      if(label)existing.delete(selectId);
      else{
        label=el('label','new-effort sdk-native-setting');label.title=title;
        select=document.createElement('select');select.id=selectId;select.setAttribute('aria-label',title);
        label.append(el('span','visually-hidden',title),select);holder.append(label);
        select.onchange=async()=>{
          const generation=view.generation,value=select.value;
          try{
            await this.settings(view,{[key]:key==='fast_mode'?value==='true':value});
          }catch(error){if(view.current(generation))view.setNotice(`设置失败：${error.message}`,'blocked');}
        };
      }
      const names=source==='droid' && key==='mode'?{auto:'执行模式',spec:'计划模式'}:source==='droid' && key==='autonomy_level'?{off:'自主权限关闭',low:'低自主权限',medium:'中自主权限',high:'高自主权限'}:key==='fast_mode'?{false:'Fast 关闭',true:'Fast 开启'}:{default:key==='permission_mode'?'默认权限':'默认模式',autopilot:'Autopilot',acceptEdits:'允许编辑','accept-edits':'允许编辑',plan:'计划模式',auto:'自动模式',dontAsk:'不询问',bypassPermissions:'跳过审批',build:'构建',general:'通用',explore:'探索'};
      const nextOpts=[];
      for(const option of options){const id=typeof option==='string'?option:option.id || option.name;nextOpts.push([id,names[id] || (typeof option==='string'?option:option.name || option.label || id)]);}
      if(current && !nextOpts.some(([id])=>id===current))nextOpts.push([current,names[current] || current]);
      const optSig=JSON.stringify(nextOpts);
      if(select.dataset.options!==optSig){
        select.replaceChildren();
        for(const [id,text] of nextOpts)select.add(new Option(text,id));
        select.dataset.options=optSig;
      }
      ConversationUI.selectValue(select,current || '',`选择${title}`);select.title=`${title}：${select.selectedOptions[0]?.textContent || '未返回'}`;
      select.disabled=pending;
      label.hidden=!options.length && !current;
      nextSettings.push(label);
    }
    for(const stale of existing.values())stale.remove();
    view.nativeSettings=nextSettings;
    sdkSyncSettings(view);
  },
  async settings(view, values) {
    values = view.modelSettings(values);
    const generation=view.generation,choices=Object.fromEntries(Object.entries(values).map(([key,value])=>[key,String(value)]));
    await sdkUpdateSettings(view,choices,()=>view.request('api/native-sdk/settings',{
      uid:view.uid,...values,request_id:crypto.randomUUID(),
    }),()=>{view.paintPickers();this.paintSettings(view);this.layout(view);});
    if(!view.current(generation))return;
    // The authoritative snapshot may confirm before this receipt. Then no later
    // snapshot repaints the status, so restore it now instead of a stale notice.
    const agy=view.uid.startsWith('agy:');
    if(view.settingChoices.size || agy && view.running)view.setNotice(agy?'设置已保存；当前运行回合保持原设置':'设置已交给原生确认');
    else view.paintStatus();
  },
  paintQueue(view) {
    const holder=view.pane.querySelector('.sdk-queue');
    if(holder._nativeQueueBusy)return;
    const nativeRows=view.raw?.queue || [],bridgeRows=view.raw?.input_queue || [],steers=view.raw?.steers || [];
    // Transcript admission can precede the write receipt (notably Kiro's
    // begin_turn). Join the two projections by this delivery's identity, never
    // by text, and repaint even when only the transcript has changed.
    const userDeliveries=new Set((view.items || []).filter(item=>item.role==='user' && item.request_id).map(item=>item.request_id));
    const signature=JSON.stringify([nativeRows,bridgeRows,steers,[...userDeliveries],view.running,view.capabilities.queue_steer]);if(holder.dataset.state===signature)return;holder.dataset.state=signature;holder.replaceChildren();
    const generation=view.generation,uid=view.uid;
    const labels={preparing:'准备输入',queued:'排队中',sending:'正在发送…',accepted:'原生已接收',failed:'发送失败',unknown:'处理结果待核对'};
    const unknownReason=item=>item.observed_consumed || item.consumption==='observed'
      ? '原生已读取这条输入，但执行结果尚未确认。'
      : '尚未确认原生是否收到或处理了这条输入。';
    const inputText=item=>item.edit_text ?? item.text ?? item.prompt?.text ?? item.id;
    const cardFor=(item,title)=>{const card=el('div','msg sdk-request'),body=el('div','mb');card.dataset.queueId=item.id;body.append(el('p','sdk-request-title',title),el('pre','',inputText(item)));card.append(body);holder.append(card);return body;};
    // Admission/delivery progress belongs to the user's message, like Codex's
    // queue. It is not an assistant request and does not prove consumption.
    const userCardFor=(item,title)=>{
      const {card,body}=ConversationUI.queuedMessage(inputText(item),title,
        {uid,agent:view.agent,className:'sdk-queued'});
      card.dataset.queueId=item.id;holder.append(card);return body;
    };
    const actionsFor=body=>{let actions=body.querySelector('.sdk-request-actions');if(!actions){actions=el('div','sdk-request-actions');body.append(actions);}return actions;};
    const operation=(body,item,method,title,extra={},edit=false)=>{
      const queued=body.parentElement.classList.contains('sdk-queued');
      const button=el('button',queued?'queued-send-dismiss':'btn',title);button.type='button';
      (queued?body.parentElement.querySelector('.sdk-queue-actions'):actionsFor(body)).append(button);
      button.onclick=async()=>{
        if(method==='queue_retry' && item.state==='unknown' && !await appConfirm(`重新发送这条输入？\n\n${unknownReason(item)}重新发送会再次提交整条输入，可能重复执行。请先核对会话历史。`,{ok:'重新发送'}))return;
        if(method==='queue_cancel' && item.state==='unknown' && !await appConfirm('请先核对会话历史。移除此记录不会停止原生任务，后续排队输入将继续。',{ok:'移除记录'}))return;
        if(!view.current(generation))return;
        holder._nativeQueueBusy=true;
        holder.querySelectorAll('button').forEach(control=>control.disabled=true);
        try{
          const cancel=()=>view.request(`api/native-sdk/${method}`,{uid,id:item.id,...extra,...(item.state==='unknown'?{confirmed:true}:{}),request_id:crypto.randomUUID()});
          if(edit)await CodexSdkCompose.recall(view,item.edit_text ?? item.text ?? item.prompt?.text ?? '',cancel);
          else await cancel();
        }
        catch(error){if(view.current(generation))view.setNotice(error.message,'blocked');}
        finally{holder._nativeQueueBusy=false;if(view.current(generation)){delete holder.dataset.state;this.paintQueue(view);}}
      };
    };
    for(const item of bridgeRows){
      const delivery=item.delivery_id || (!item.attempts ? item.request_id : null);
      if(item.state==='accepted' || (item.state==='sending' && delivery && userDeliveries.has(delivery))){
        const receipt=el('div','native-message-state',labels[item.state]);
        receipt.dataset.queueId=item.id;receipt.dataset.queueOrigin='bridge';
        receipt.title=item.state==='accepted'?'仅表示原生已接收输入，不表示已处理或已完成':'输入正文已展示，原生投递尚未确认';
        holder.append(receipt);continue;
      }
      const ordinaryInput=['preparing','queued','sending'].includes(item.state);
      const body=(ordinaryInput?userCardFor:cardFor)(item,labels[item.state] || item.state);body.parentElement.dataset.queueOrigin='bridge';
      if(item.error)body.append(el('p','sdk-control-note',item.error));
      if(item.state==='unknown')body.append(el('p','sdk-control-note',item.operation==='steer'?'引导处理结果尚未确认。请核对历史；移除记录不会停止原生任务。':`${unknownReason(item)}后续输入已暂停。核对历史后，可移除记录以继续，或重新发送这条输入。`));
      if(['failed','unknown'].includes(item.state) && item.operation!=='steer')operation(body,item,'queue_retry',item.state==='unknown'?'重新发送':'重试');
      if(['queued','failed','unknown'].includes(item.state))operation(body,item,'queue_cancel',item.state==='unknown'?'移除记录':'撤销');
      if(item.state==='queued')operation(body,item,'queue_cancel','撤回编辑',{},true);
      if(item.state==='queued' && item.operation==='turn' && view.running && view.capabilities.queue_steer && (!item.attachments?.length || ['droid','agy'].includes(view.uid.split(':')[0])))operation(body,item,'queue_steer','立即引导');
      if(['failed','unknown'].includes(item.state) && (item.edit_text ?? item.text)){const restore=el('button','btn','恢复文字');restore.type='button';restore.onclick=()=>{if(!view.current(generation))return;view.input.value=[view.input.value,inputText(item)].filter(Boolean).join('\n');view.fitInput();CodexSdkCompose.input(view);view.input.focus();view.setNotice('文字已追加到输入框；队列记录保留');};actionsFor(body).append(restore);}
    }
    for(const item of nativeRows){const body=userCardFor(item,item.delivery==='steer'?'等待引导':'排队中');body.parentElement.dataset.queueOrigin='native';
      if(view.capabilities.queue_cancel)operation(body,item,'queue_cancel','撤回编辑',{},true);
      for(const [method,title,extra] of [['queue_cancel','撤销',{}],['queue_update','立即引导',{delivery:'steer'}]])if(view.capabilities[method] && !(method==='queue_update' && item.delivery==='steer'))operation(body,item,method,title,extra);
    }
    const steerLabels={dispatching:'正在提交引导',native_queued:'原生已接收引导',native_received:'原生已登记引导',rejected:'引导被拒绝',uncertain:'引导结果待核对',connection_closed:'引导连接已关闭'};
    const composerSteer=view.uid.startsWith('droid:') || view.uid.startsWith('agy:');
    for(const item of steers){
      if(composerSteer && !['dispatching','native_queued'].includes(item.status))continue;
      const id=item.request_id || item.id,title=steerLabels[item.status] || '引导回执';
      const note='原生接收回执不能确认输入已被处理；临近任务结束时可能另开一轮。';
      if(composerSteer && id){
        if(bridgeRows.some(row=>row.request_id===id || row.delivery_id===id))continue;
        if(userDeliveries.has(id)){
          const receipt=el('div','native-message-state',title);receipt.dataset.queueId=item.id || id;receipt.dataset.queueOrigin='steer';
          receipt.title=[note,item.error].filter(Boolean).join('\n');holder.append(receipt);continue;
        }
      }
      const body=cardFor({...item,id:item.id || item.request_id},title);body.parentElement.dataset.queueOrigin='steer';body.append(el('p','sdk-control-note',note));if(item.error)body.append(el('p','sdk-control-note',item.error));
    }
  },
  rewindResult(view,response) {
    const native=response.native || response.raw?.native || {};
    const failed=!!(native.failedRestoreCount || native.failedDeleteCount);
    view.nativeActionResult={failed,text:`${failed?'回退部分失败':'回退完成'}：恢复 ${native.restoredCount ?? '未知'} 个文件，删除 ${native.deletedCount ?? '未知'} 个文件；恢复失败 ${native.failedRestoreCount ?? '未知'}，删除失败 ${native.failedDeleteCount ?? '未知'}。已在新会话中继续，原会话保留。`};
    view.paintStatus();
  },
  async droidRewind(view,body) {
    const generation=view.generation;
    if(!body.before){
      const modal=CodexSdkActions.form('选择回退位置','将从所选消息之前创建新会话，原会话历史保留。','下一步');
      const select=document.createElement('select');select.setAttribute('aria-label','回退消息');
      const seen=new Set();
      for(const item of [...(view.historyData?.messages || []),...view.items].filter(item=>item.role==='user')){
        const id=this.messageIdentity(view,item);if(id && !seen.has(id)){seen.add(id);select.add(new Option((item.text || id).slice(0,100),id));}
      }
      modal.fields.append(select);modal.form.onsubmit=e=>{e.preventDefault();if(select.value)modal.finish(select.value);};
      const before=await modal.answer;if(!before)return null;body={...body,before};
    }
    const response=await view.request('api/native-sdk/rewind_points',{uid:view.uid,before:body.before});
    if(!view.current(generation))return null;
    const info=response.rewind_info || response.raw?.rewind_info || {};
    const modal=CodexSdkActions.form('回退对话与文件','从该消息之前创建新会话。勾选要恢复或删除的工作区文件；未勾选的文件保持现状，原会话历史保留。','创建回退分支');
    const selected=[];
    for(const [key,rows,label] of [['files_to_restore',info.availableFiles,'恢复'],['files_to_delete',info.createdFiles,'删除']]){
      for(const file of rows || []){
        const row=el('label','setting-row continue-session-option'),input=document.createElement('input');input.type='checkbox';input.setAttribute('aria-label',`${label} ${file.filePath}`);
        const text=el('span','',`${label}：${file.filePath}`);text.style.overflowWrap='anywhere';row.append(text,input);modal.fields.append(row);selected.push({key,file,input});
      }
    }
    if(!selected.length)modal.fields.append(el('p','','此位置没有可恢复或删除的文件；仅回退对话。'));
    for(const file of info.evictedFiles || [])modal.fields.append(el('p','',`无法恢复：${file.filePath}（${file.reason}）`));
    modal.form.onsubmit=e=>{e.preventDefault();modal.finish({...body,files_to_restore:selected.filter(x=>x.key==='files_to_restore' && x.input.checked).map(x=>x.file),files_to_delete:selected.filter(x=>x.key==='files_to_delete' && x.input.checked).map(x=>x.file)});};
    return modal.answer;
  },
  async action(view,method,body={}) {
    if(method==='steer' && ['kiro','droid','agy'].includes(view.uid.split(':')[0]))return view.send('steer');
    const generation=view.generation,droid=view.uid.startsWith('droid:');
    if(droid && view.nativeActionPending){view.setNotice('会话操作正在进行，请稍候。');return false;}
    if(droid && method==='fork' && (body.before!=null || body.prompt_index!=null) && !view.capabilities.fork_before){view.setNotice('当前原生接口尚未提供按消息分支，输入已保留。','blocked');return false;}
    const kiroAllowed=view.uid.startsWith('kiro:') && method==='compact' && view.capabilities.compact!==false;
    if(!this.supportsAction(view,method) && !kiroAllowed){view.setNotice('当前原生接口未提供此操作','blocked');return false;}
    if(droid){view.nativeActionPending=true;this.paint(view);}
    try{
      if(droid && method==='compact' && view.capabilities.compact_creates_session && !await appConfirm('压缩当前上下文？原生将压缩会话记录；若生成新会话，将切换到新会话继续。',{ok:'压缩'}))return false;
      if(method==='fork'){
        const kiro=view.uid.startsWith('kiro:') && body.messageId;
        const text=kiro ? body.hide_source
          ? '回退并从所选轮次继续？新会话保留该轮及回复，原会话会隐藏。原会话内容和工作区文件保持不变。'
          : '从所选轮次创建分支？新会话保留该轮及回复，原会话保持不变。'
          : body.after_reply
            ? '保留到此回复创建分支？将在新会话中继续，此回复之后的对话不会进入新分支，原会话保持不变。'
            : body.before!=null || body.prompt_index!=null
              ? droid ? '从这条消息之前创建分支？仅分支对话，原会话和工作区文件保持不变。' : '从这条消息之前创建分支？原会话保持不变。'
              : '创建当前会话的分支？将在新会话中继续，原会话保持不变。';
        if(!await appConfirm(text,{ok:kiro && body.hide_source?'回退':'分支'}))return false;
      }
      if((method==='revert' && body.message_id) || (!droid && method==='rewind' && body.prompt_index!=null)){
        if(!await appConfirm('回退到这条消息之前？消息内容将追加回输入框，工作区文件保持原样。',{ok:'回退'}))return false;
      }
      if(!view.current(generation))return false;
      if(method==='steer'){
        const modal=CodexSdkActions.form('引导当前任务','原生会在可插入时处理；临近任务结束时可能另开一轮。普通发送仍进入队列。','发送引导');
        const input=document.createElement('textarea');input.setAttribute('aria-label','引导内容');modal.fields.append(input);
        modal.form.onsubmit=event=>{event.preventDefault();if(input.value.trim())modal.finish({text:input.value});};body=await modal.answer;if(!body)return false;
      }
      if(method==='permissions'){
        const modal=CodexSdkActions.form('权限规则','编辑 OpenCode 原生 permissions JSON 数组（例如 [{"permission":"bash","pattern":"*","action":"ask"}]）；保存后由原生服务校验。','保存');const input=document.createElement('textarea');input.setAttribute('aria-label','权限规则 JSON');input.value=JSON.stringify(view.raw?.permissions || [],null,2);modal.fields.append(input);
        modal.form.onsubmit=event=>{event.preventDefault();try{modal.finish(JSON.parse(input.value));}catch{modal.error.textContent='请输入有效 JSON';}};
        const permissions=await modal.answer;if(permissions==null)return false;body={permissions};method='settings';
      }
      if(method==='command'){
        const response=await view.request('api/native-sdk/commands',{uid:view.uid});const rows=response.commands || response.raw?.commands || response.raw?.data || response.raw || [];
        const modal=CodexSdkActions.form('原生命令','选择原生服务提供的命令。','运行');const select=document.createElement('select');select.setAttribute('aria-label','原生命令');for(const row of Array.isArray(rows)?rows:[])select.add(new Option(row.name,row.name));const input=document.createElement('textarea');input.setAttribute('aria-label','命令参数');modal.fields.append(select,input);modal.form.onsubmit=event=>{event.preventDefault();if(select.value)modal.finish({name:select.value,text:input.value});};body=await modal.answer;if(!body)return false;
      }
      if(method==='shell' && !body.command){const modal=CodexSdkActions.form('运行命令','在会话工作目录由原生接口直接运行 Shell 命令，不经过模型推理，使用该来源的实际权限策略。','运行');const input=document.createElement('textarea');input.setAttribute('aria-label','命令');modal.fields.append(input);modal.form.onsubmit=event=>{event.preventDefault();if(input.value.trim())modal.finish(input.value);};body.command=await modal.answer;if(!body.command)return false;}
      if(method==='revert' && !body.message_id){
        const modal=CodexSdkActions.form('回退对话','选择要回退到的消息之前；不回滚工作区文件。','回退');
        const select=document.createElement('select');select.setAttribute('aria-label','回退消息');
        const messages=new Map();
        for(const item of [...(view.historyData?.messages || []),...view.items]){
          const id=this.messageIdentity(view,item);
          if(id && !messages.has(id)){messages.set(id,item);select.add(new Option((item.text || item.content || id).slice(0,100),id));}
        }
        modal.fields.append(select);modal.form.onsubmit=event=>{event.preventDefault();if(select.value)modal.finish(select.value);};
        body.message_id=await modal.answer;if(!body.message_id)return false;
        const item=messages.get(body.message_id);body.restore_text=item?.text || item?.content || '';
      }
      if(droid && method==='rewind'){body=await this.droidRewind(view,body);if(!body)return false;}
      if(!droid && method==='rewind' && body.prompt_index==null){const response=await view.request('api/native-sdk/rewind_points',{uid:view.uid});const points=response.rewind_points || response.raw?.rewind_points || response.raw?.points || [];const modal=CodexSdkActions.form('回退对话','仅回退会话历史，不回滚工作区文件。','回退');const select=document.createElement('select');select.setAttribute('aria-label','回退消息');for(const point of points)select.add(new Option(point.prompt_preview || point.preview || String(point.prompt_index),point.prompt_index));modal.fields.append(select);modal.form.onsubmit=event=>{event.preventDefault();if(select.value!=='')modal.finish(Number(select.value));};body.prompt_index=await modal.answer;if(body.prompt_index==null)return false;}
      if(method==='revert_commit' && !await appConfirm('提交回退会永久移除被回退的对话记录；工作区文件不会回滚。',{ok:'提交回退'}))return false;
      if(!view.current(generation))return false;
      if(method==='compact'){
        view.raw={...(view.raw || {}),status:'compacting',compact:{state:'compacting'}};
        view.paintStatus();
      }
      if(method==='revert' && view.uid.startsWith('claude:')){
        const messages=[...(view.historyData?.messages || []),...view.items];
        body.last_seen_user_message_uuid=messages.map(item=>this.messageIdentity(view,item)).filter(Boolean).at(-1);
      }
      const {restore_text,hide_source,command_text,after_reply,...nativeBody}=body;
      const sourceUid=view.uid;
      const response=await view.request(`api/native-sdk/${method}`,{uid:view.uid,...nativeBody,request_id:crypto.randomUUID()});
      if(!view.current(generation)){
        // Inventory can select a native continuation before its receipt arrives.
        // Finish only that already-selected handoff; never undo user navigation.
        if(droid && (method==='compact' || method==='rewind' || method==='fork' && body.before!=null) && response.uid && response.uid!==sourceUid && S.sel===response.uid && SdkView.uid===response.uid && !SdkView.agent && (response.accepted ?? response.raw?.accepted)!==false){
          await SdkView.startSession(response.uid);
          if(SdkView.uid===response.uid && !SdkView.openFailed)await SdkView.refreshHistory(SdkView.generation,true);
          if(body.restore_text && SdkView.uid===response.uid)await CodexSdkActions.restoreInput(SdkView.active,body.restore_text);
          if(method==='rewind' && SdkView.uid===response.uid)this.rewindResult(SdkView.active,response);
        }
        return true;
      }
      if((response.accepted ?? response.raw?.accepted)===false)throw new Error(response.error || response.raw?.error || '原生未确认接收，请核对操作记录');
      if(droid && response.agent && response.agent!==view.agent){
        if(command_text && view.input?.value===command_text){view.input.value='';view.fitInput();CodexSdkCompose.input(view);}
        await loadSessions(true,true);
        await openSession(response.uid,response.agent);
        await SdkView.startSession(response.uid,response.agent);
        if(SdkView.uid===response.uid && SdkView.agent===response.agent && !SdkView.openFailed)
          await SdkView.refreshHistory(SdkView.generation,true);
        if(body.restore_text && SdkView.uid===response.uid && SdkView.agent===response.agent)
          await CodexSdkActions.restoreInput(SdkView.active,body.restore_text);
        if(method==='rewind' && SdkView.uid===response.uid && SdkView.agent===response.agent)this.rewindResult(SdkView.active,response);
        return true;
      }
      if(method==='fork' || droid && ['compact','rewind'].includes(method) && response.uid && response.uid!==sourceUid){
        if(droid && command_text && view.input?.value===command_text){view.input.value='';view.fitInput();CodexSdkCompose.input(view);}
        await CodexSdkActions.openFork(view,response);
        if(['kiro','droid','agy','codebuddy'].includes(response.session?.source) && SdkView.uid===response.uid){
          await SdkView.startSession(response.uid);
          if(SdkView.uid===response.uid && !SdkView.openFailed)
            await SdkView.refreshHistory(SdkView.generation,true);
        }
        if(hide_source && SdkView.uid===response.uid && !SdkView.openFailed)
          await setForkParentVisibility([sourceUid],false);
        if (body.restore_text && SdkView.uid === response.uid) await CodexSdkActions.restoreInput(SdkView.active,body.restore_text);
        if(droid && method==='rewind' && SdkView.uid===response.uid)this.rewindResult(SdkView.active,response);
        return true;
      }
      if(['revert','unrevert','revert_commit','rewind'].includes(method)){
        await view.refreshHistory(generation,true);
        if(body.restore_text && view.current(generation))await CodexSdkActions.restoreInput(view,body.restore_text);
      }
      if(method==='compact'){
        if(droid){await view.refreshHistory(generation,true);if(!view.current(generation))return true;void this.loadUsage(view,false);}
        if(view.raw?.compact?.state==='compacting')view.raw={...view.raw,status:'idle',compact:response.raw?.compact || response.compact || {state:'success'}};
        view.paintStatus();
      }else view.setNotice('原生操作已接受');return true;
    }catch(error){
      if(view.current(generation)){
        if(method==='compact' && view.raw?.compact?.state==='compacting')view.raw={...view.raw,status:'idle',compact:{state:'failed',error:error.message}};
        view.setNotice(`操作失败：${error.message}`,'blocked');
      }
      return false;
    }finally{
      if(droid && view.current(generation)){view.nativeActionPending=false;this.paint(view);}
    }
  },
  async command(view,text) {
    const match=/^\/(\S+)(?:\s+([\s\S]*))?$/.exec(text.trimEnd());if(!match && !text.startsWith('!'))return false;
    const command=match?.[1] || 'shell',arg=(match?.[2] || (match?'':text.slice(1))).trim();
    let success=false;
    if(['usage','context','status','help'].includes(command)){
      if(command==='usage' || command==='context')void this.loadUsage(view,true);
      if(command==='help')view.setNotice('网页命令：/usage、/context、/status、/model ID、/effort 档位、/mode 模式、/plan、/stop、/new、/rename 名称、/fork、/compact、/revert、/rewind、/shell 命令。会话操作按原生能力开放。');
      else if(command==='status'){const menu=view.pane.querySelector('#sdk-status-menu');menu.showPopover();ConversationUI.placePopover(menu,view.notice);}
      // /usage and /context refresh the always-visible footer; no menu opens.
      success=true;
    }else if(command==='new'){
      const source=view.uid.split(':')[0];
      document.querySelector('#new-session')?.click();
      const radio=document.querySelector(`#new-session-form input[name="new-source"][value="${CSS.escape(source)}"]`);
      if(radio && !radio.disabled)radio.click();
      success=true;
    }else if(command==='rename'){return this.rename(view,text,arg);
    }else if(command==='stop'){await view.interrupt();success=true;}
    else if(['model','effort','mode'].includes(command)){
      if(!arg)view.setNotice(`用法：/${command} 值`,'blocked');
      else {await this.settings(view,{[command]:arg});success=true;}
    }else if(command==='plan'){
      await this.settings(view,{mode:arg || (view.uid.startsWith('droid:')?'spec':'plan')});
      success=true;
    }else if(['fork','compact','revert','rewind','shell'].includes(command))success=await this.action(view,command,{...(command==='shell'?{command:arg}:command==='fork'&&arg?(view.uid.startsWith('kiro:')?{messageId:arg}:{before:arg}):{}),...(view.uid.startsWith('droid:')?{command_text:text}:{})});
    else view.setNotice(`此网页尚未映射 /${command}，输入已保留。普通文字请去掉开头的 /。`,'blocked');
    if(success && view.input?.value===text){view.input.value='';view.fitInput();CodexSdkCompose.input(view);}
    return true;
  },
  async rename(view,text,argument) {
    const uid=view.uid,generation=view.generation,input=view.input,requestId=crypto.randomUUID();
    const current=()=>view.current(generation) && view.uid===uid;
    const audit=(phase,data={})=>browserAuditEvent('sdk.rename.'+phase,{source:uid.split(':')[0],...data},{uid,requestId});
    audit('invoked',{dialog:!argument});
    if(view.capabilities.rename!==true){
      view.setNotice('当前原生机器接口尚未提供改名；输入已保留。','blocked');
      audit('finished',{outcome:'unsupported'});return true;
    }
    try{
      let title=argument;
      if(!title){
        const modal=CodexSdkActions.form('重命名会话','输入此会话的新名称。','保存');
        const field=document.createElement('input');
        field.className='new-model-search';field.setAttribute('aria-label','会话名称');field.required=true;
        field.value=S.sessions.find(session=>session.uid===uid)?.title || view.raw?.title || '';
        modal.fields.append(field);
        modal.form.onsubmit=event=>{event.preventDefault();if(field.value.trim())modal.finish(field.value.trim());};
        field.focus();field.select();title=await modal.answer;
      }
      if(!title || !current()){audit('finished',{outcome:title?'session_changed':'cancelled'});return true;}
      audit('requested',{title_length:title.length});
      await view.request('api/native-sdk/rename',{uid,title,request_id:requestId});
      audit('finished',{outcome:'succeeded'});
      if(current()){
        view.setNotice('会话已重命名');
        // Refreshing the inventory can replace the view. Consume only this
        // successful Droid command before that replacement, preserving edits.
        if(uid.startsWith('droid:')){
          if(input?.value===text){input.value='';view.fitInput();CodexSdkCompose.input(view);}
          if(!await persistComposerDraft(uid))throw new Error('会话已重命名，但输入草稿尚未保存，请保留当前页面');
        }
      }
      await loadTermList(true);
      await loadSessions(true,true);
      if(current() && input?.value===text){input.value='';view.fitInput();CodexSdkCompose.input(view);}
    }catch(error){
      audit('finished',{outcome:'failed'});
      if(current())view.setNotice(`改名失败：${error.message}`,'blocked');
    }
    return true;
  },
  messageIdentity(view,item) {
    if (!item || item.role !== 'user') return null;
    const source=view.uid.split(':')[0];
    if(source==='grok'){
      const index=item.prompt_index ?? (/^prompt:(\d+)$/.exec(item.turn_id || '')?.[1]);
      return index == null ? null : `prompt:${index}`;
    }
    if(source==='agy'){
      const index=item.native?.step_index ?? (/^step:(\d+)$/.exec(item.turn_id || '')?.[1])
        ?? (/:step:(\d+)$/.exec(item.native_id || '')?.[1]);
      return index == null ? null : `step:${index}`;
    }
    if(source==='opencode')return item.native_id || (/^message:(.+)$/.exec(item.turn_id || '')?.[1]) || null;
    if(source==='kiro')return item.message_id || item.id || null;
    if(source==='claude')return item.native_id || item.turn_id || item.id || null;
    if(source==='codebuddy'||source==='droid')return item.native_id || null;
    return null;
  },
  decorate(view,node,item) {
    if(item?.role!=='user' || node.querySelector(':scope > .message-header .sdk-bubble-menu'))return;
    const identity=this.messageIdentity(view,item),grok=view.uid.startsWith('grok:'),droid=view.uid.startsWith('droid:');
    if(!identity)return;
    if(view.uid.startsWith('kiro:')){
      if(item.message_id && view.capabilities.fork && view.capabilities.fork_at_message)
        SdkUI.decorateUserBubble(view,node,[
          ['回退',()=>this.action(view,'fork',{messageId:item.message_id,hide_source:true}),
            '回退到此轮：保留本轮及助手回复创建新会话，并隐藏原会话（原会话与工作区文件不变）'],
          ['分支',()=>this.action(view,'fork',{messageId:item.message_id}),
            '从本轮创建分支：保留本轮及助手回复在新会话继续，原会话保持可见'],
        ]);
      return;
    }
    const items=[];
    for(const [method,label,tooltip] of [
      ['fork','从此处之前分支','从此处之前创建分支：在此消息之前分出新会话并带回此条输入，原会话与工作区文件保持不变'],
      [grok || droid?'rewind':'revert',droid?'回退对话与文件':'回退到此处之前',droid
        ? '回退对话与文件：从此消息之前创建新分支，可勾选要恢复或删除的工作区文件，原会话保留'
        : '回退到此消息前：移除此条及之后的对话并将消息放回输入框，不修改工作区文件'],
    ]){
      if(!view.capabilities[method] || method==='fork' && droid && !view.capabilities.fork_before)continue;
      items.push([label,async()=>{
        await this.action(view,method,{...(grok?{prompt_index:Number(identity.slice(7))}:method==='fork' || droid?{before:identity}:{message_id:identity}),restore_text:item.text || item.content || ''});
      },tooltip]);
    }
    SdkUI.decorateUserBubble(view,node,items);
  },
  decorateAssistant(view,node) {
    SdkUI.decorateUserBubble(view,node,[
      ['从此处创建分支',()=>this.assistantFork(view,node),
        '保留到此回复创建分支：携带截至此回复的对话在新会话继续，原会话保持不变'],
    ]);
  },
  async assistantFork(view,node) {
    const {nextUser,hasGap}=SdkUI.nextUserBubble(view,node);
    if(hasGap){
      view.setNotice('此回复后存在折叠的历史缺口；请先展开紧随其后的历史记录再创建分支。','blocked');
      return false;
    }
    if(!nextUser){
      if(view.running){
        view.setNotice('当前回合尚未结束；请等待回复完成后再创建分支。','blocked');
        return false;
      }
      return this.action(view,'fork',{});
    }
    const identity=this.messageIdentity(view,nextUser._sdkMessage);
    if(!identity){
      view.setNotice('后续消息尚未分配原生分支位置；可在最后一轮回复上创建分支。','blocked');
      return false;
    }
    return this.action(view,'fork',view.uid.startsWith('grok:')
      ? {prompt_index:Number(identity.slice(7)),after_reply:true}
      : {before:identity,after_reply:true});
  },
  elicitation(view,request) {
    const card=el('div','msg sdk-request');card.dataset.sdkRequest=request.key;const body=el('div','mb');body.append(el('p','sdk-request-title',request.title || 'MCP 请求'),el('p','',request.message || request.description || ''));
    const readers=[];
    if(request.mode==='url'){const link=document.createElement('a');try{const url=new URL(request.url);if(['http:','https:'].includes(url.protocol)){link.href=url.href;link.target='_blank';link.rel='noopener noreferrer';link.textContent='打开授权页面';body.append(link);}}catch{}}
    else {
      const properties=request.requested_schema?.properties || {},required=request.requested_schema?.required || [];
      const keys=[...new Set([...(request.property_order || []),...Object.keys(properties)])].filter(key=>properties[key]);
      for(const key of keys){
        const schema=properties[key],label=el('label','sdk-control-section',schema.title || key);let input;
        const typed=Array.isArray(schema.enum)?schema.enum:null;
        if(typed){input=document.createElement('select');input.add(new Option('选择',''));typed.forEach((choice,index)=>input.add(new Option(String(choice),String(index))));}
        else{input=document.createElement('input');input.type=schema.type==='boolean'?'checkbox':schema.type==='integer'||schema.type==='number'?'number':'text';if(schema.type==='integer')input.step='1';}
        if(input.type!=='checkbox')input.required=required.includes(key);
        for(const [attribute,value] of [['min',schema.minimum],['max',schema.maximum],['minLength',schema.minLength],['maxLength',schema.maxLength],['pattern',schema.pattern]])if(value!=null)input[attribute]=value;
        if(schema.default!=null){if(input.type==='checkbox')input.checked=schema.default===true;else input.value=typed?String(typed.indexOf(schema.default)):String(schema.default);}
        input.setAttribute('aria-label',schema.title || key);label.append(input);body.append(label);
        readers.push(()=>{
          if(!input.reportValidity())throw new Error('请完成必填字段');
          if(input.value==='' && input.type!=='checkbox' && !required.includes(key))return null;
          return [key,typed?typed[Number(input.value)]:schema.type==='boolean'?input.checked:schema.type==='number'||schema.type==='integer'?Number(input.value):input.value];
        });
      }
    }
    for(const [label,action] of [['提交','accept'],['拒绝','decline'],['取消','cancel']]){const button=el('button','btn',label);button.type='button';button.onclick=()=>{try{view.answer(request,{action,...(action==='accept' && request.mode!=='url'?{content:Object.fromEntries(readers.map(read=>read()).filter(Boolean))}:{})},card);}catch(error){view.setNotice(error.message,'blocked');}};body.append(button);}card.append(body);return card;
  },
};
