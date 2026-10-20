'use strict';

// Native capabilities share presentation, never another provider's wire semantics.
const NativeSdkControls = {
  mount(view) {
    view.settingChoices = new Map(); view.nativeSettingValues = new Map();
    view.usageRead=null;view.usageRetryTimer=0;view.usageRetryDelay=1000;
    view.usageData=UsageUI.remembered(view.uid,view.agent) || {};
    view.usageData.quota=UsageUI.quota(view.uid) || view.usageData.quota;
    view.quotaRevision=0;
    view.nativeSettings=[];
    const menu = view.pane.querySelector('#sdk-more-menu');
    menu.classList.add('sdk-native-menu');
    const usageMenu = el('section','sdk-inline-usage'); usageMenu.id='sdk-usage-menu'; usageMenu.setAttribute('aria-label','用量');
    const detail = el('div','sdk-control-section'); detail.id='native-usage-detail';
    const refresh = el('button','btn','刷新'); refresh.type='button'; refresh.id='native-usage-refresh'; refresh.setAttribute('aria-label','刷新用量');
    refresh.onclick=()=>this.loadUsage(view,true);
    const heading=el('div','sdk-usage-heading'); heading.append(el('h3','','用量'),refresh); usageMenu.append(heading,detail);
    const actions = el('div',''); actions.id = 'sdk-actions'; menu.append(actions);
    for (const [method,label] of [['fork','分支会话'],['steer','引导当前任务'],['compact','压缩上下文'],['revert','回退对话'],['unrevert','撤销回退'],['revert_commit','提交回退'],['rewind','回退对话'],['shell','运行命令'],['command','原生命令'],['permissions','权限规则']]) {
      const button = el('button','new-model-option sdk-action'); button.type = 'button';button.dataset.sdkAction = method;button.title=label;button.setAttribute('aria-label',label);
      button.innerHTML=uiIcon(({fork:'fork',compact:'compact',revert:'undo',unrevert:'undo',revert_commit:'continue',rewind:'undo',steer:'continue',shell:'terminal',command:'list',permissions:'settings'})[method]);button.append(el('span','sdk-action-label',label));
      button.onclick = () => { view.closeToolbarMenus(); void this.action(view,method); }; actions.append(button);
    }
    const settings = el('div','sdk-control-section'); settings.id = 'native-sdk-settings'; menu.append(settings);
    menu.append(usageMenu);
    this.paint(view);
    // Account snapshots are independent of native startup; never wait for /open.
    if(['agy','claude'].includes(view.uid.split(':')[0])) {
      const generation=view.generation,revision=view.quotaRevision,read=new AbortController();view.quotaRead=read;
      void UsageUI.readQuota(view.uid,read.signal).then(quota=>{
        if(!view.current(generation) || revision!==view.quotaRevision || !quota)return;
        view.usageData.quota=quota;this.paintUsage(view);
      }).catch(()=>{});
    }
  },
  close(view) {
    if(view.uid)this.rememberUsage(view);
    view.quotaRead?.abort();view.quotaRead=null;
    clearTimeout(view.usageRetryTimer);view.usageRetryTimer=0;
    view.usageRead?.abort();view.usageRead=null;
    CodexSdkActions.close();
  },
  layout(view) {
    if (!view.pane) return;
    const gauges=view.pane.querySelector('.sdk-gauges'),menu=view.pane.querySelector('#sdk-usage-menu');
    if (!gauges || !menu) return;
    const more=view.pane.querySelector('#sdk-more-menu');
    SdkUI.layoutFooter(view, ['.sdk-model', '.sdk-effort'].map(selector => view.pane.querySelector(selector))
      .concat(view.nativeSettings || [],[...view.pane.querySelectorAll('[data-sdk-action]')]));
    for (const [popover,trigger] of [[menu,'#sdk-more'],[more,'#sdk-more']])
      if (popover.matches(':popover-open')) ConversationUI.placePopover(popover,view.pane.querySelector(trigger));
    view.updateStatusDisclosure();
  },
  async loadUsage(view,refresh) {
    const generation=view.generation;
    if (!view.uid || view.loadingUsage) return;
    clearTimeout(view.usageRetryTimer);view.usageRetryTimer=0;
    const read=new AbortController();view.usageRead=read;
    view.loadingUsage=true;
    const button=view.pane?.querySelector('#native-usage-refresh'); if (button) button.disabled=true;
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
      view.usageRead=null;view.loadingUsage=false;if(button)button.disabled=false;this.paintUsage(view);
      if(view.usageError){
        view.usageRetryTimer=setTimeout(()=>{if(view.current(generation))void this.loadUsage(view,refresh);},view.usageRetryDelay);
        view.usageRetryDelay=Math.min(view.usageRetryDelay*2,30000);
      }else view.usageRetryDelay=1000;
    } }
  },
  paint(view) {
    if (!view.pane) return;
    for (const button of view.pane.querySelectorAll('[data-sdk-action]')) button.hidden=view.capabilities[button.dataset.sdkAction] !== true && !(button.dataset.sdkAction==='permissions' && view.uid.startsWith('opencode:') && view.capabilities.settings);
    this.paintSettings(view);this.paintUsage(view);this.paintQueue(view);this.layout(view);
  },
  rememberUsage(view) {
    const data={...view.usageData};
    for(const key of ['usage','context_usage','billing','rate_limits','step_usage','usage_errors'])
      if(view.raw?.[key] && Object.keys(view.raw[key]).length)data[key]=view.raw[key];
    UsageUI.remember(view.uid,view.agent,data);
    UsageUI.rememberQuota(view.uid,data.quota);
  },
  paintUsage(view) {
    if (!view.pane) return;
    const gauges=view.pane.querySelector('.sdk-gauges'),detail=view.pane.querySelector('#native-usage-detail');if(!gauges || !detail)return;
    gauges.replaceChildren();detail.replaceChildren();
    const usage=Object.keys(view.raw?.usage || {}).length ? view.raw.usage : view.usageData?.usage || {},t=usage.tokens || usage;
    const number=(...values)=>values.find(value=>typeof value==='number' && Number.isFinite(value));
    const count=value=>value.toLocaleString(undefined,{maximumFractionDigits:0});
    const input=number(t.input,t.input_tokens,t.inputTokens),output=number(t.output,t.output_tokens,t.outputTokens);
    const total=number(t.total_tokens,t.totalTokens,input != null && output != null ? input+output : null);
    const text=(label,value)=>{const line=el('div','sdk-control-note',`${label}：${value}`);detail.append(line);};
    const gauge=(label,used,title)=>{
      if (!Number.isFinite(used)) return;
      gauges.append(UsageUI.gauge(label,used,title));text(label,title);
    };
    if(total!=null){const node=el('span','sdk-credits',`${total >= 10000 ? total.toLocaleString('en', {notation:'compact', maximumFractionDigits:1}) : count(total)} tok`);node.title=`累计 ${count(total)} tokens；不是当前上下文占用`;gauges.append(node);}
    for(const [label,value] of [['输入 Token',input],['输出 Token',output],['思考 Token',number(t.reasoning,t.thinking_tokens,t.thinkingTokens,t.reasoningTokens)],['缓存读取',number(t.cache?.read,t.cache_read_tokens,t.cacheReadInputTokens,t.cachedInputTokens,t.cachedReadTokens)],['缓存写入',number(t.cache?.write,t.cache_creation_input_tokens,t.cacheCreationInputTokens,t.cacheCreationTokens)],['累计 Token',total]])if(value!=null)text(label,count(value));
    const cost=usage.usageIsIncomplete || usage.costIsPartial ? undefined : number(usage.cost_usd,usage.total_cost_usd,typeof usage.costUsdTicks==='number'?usage.costUsdTicks/1e10:null); if(cost!=null)text('成本',`$${cost.toLocaleString(undefined,{maximumFractionDigits:6})}`);
    const context=usage.context || view.raw?.context_usage || view.usageData?.context_usage || {};
    const percent=number(context.percent,context.percentage);if(percent!=null)gauge('上下文',percent,`已用 ${percent.toFixed(1)}%${context.used_tokens!=null ? `（${count(context.used_tokens)} / ${count(context.max_tokens)} tokens）`:''}；${context.basis || '原生估计'}`);
    const quota=view.usageData?.quota;
    UsageUI.appendQuotas(gauges,quota,view.uid.split(':')[0]);
    for(const bucket of quota?.buckets || [])text(`${bucket.group || bucket.id} · ${UsageUI.window(bucket.window)}`,`剩余 ${Number.isFinite(bucket.remaining_percent)?UsageUI.percent(bucket.remaining_percent)+'%':'未知'}${bucket.resets_at ? `；${UsageUI.time(bucket.resets_at)} 重置`:''}`);
    if(quota?.error)text('额度查询',quota.error);
    const billing=view.raw?.billing || view.usageData?.billing;const b=billing?.config || billing;
    if(b){gauge('额度',b.creditUsagePercent,`本周期已用 ${b.creditUsagePercent}%${b.currentPeriod?.end ? `；${new Date(b.currentPeriod.end).toLocaleString()} 重置`:''}`);if(b.prepaidBalance?.val!=null)text('预付余额',`$${(b.prepaidBalance.val/100).toFixed(2)}`);}
    const rate=view.raw?.rate_limits || view.usageData?.rate_limits;
    for(const [window,limit] of Object.entries(rate || {})) {
      const used=number(limit?.utilization,limit?.used_percentage,limit?.usedPercent);
      if(used!=null)text(UsageUI.window(window),`已用 ${UsageUI.percent(used)}%${limit.resets_at ? `；${UsageUI.time(limit.resets_at)} 重置`:''}`);
    }
    if(usage.basis)text('统计范围',usage.basis);
    if(view.raw?.step_usage)text('最近步骤用量',JSON.stringify(view.raw.step_usage));
    for(const [key,error] of Object.entries(view.raw?.usage_errors || {}))text(`用量 ${key}`,error);
    if(view.raw?.settings?.fast_mode_state)text('Fast 状态',JSON.stringify(view.raw.settings.fast_mode_state));
    if(view.raw?.settings?.fast_mode_disabled_reason)text('Fast 不可用原因',view.raw.settings.fast_mode_disabled_reason);
    if(view.usageError)text('查询未完成',view.usageError);
    if(!detail.childElementCount)text('用量','原生尚未提供用量数据');
    const description=detail.textContent;
    gauges.title=description;gauges.setAttribute('aria-description',description);
    this.paintUsageDetail(detail,view,usage,t,total,input,output);
    const usageMenu=view.pane.querySelector('#sdk-usage-menu');
    usageMenu.hidden=!detail.childElementCount;
    this.rememberUsage(view);
    this.layout(view);
  },
  paintUsageDetail(detail,view,usage,t,total,input,output) {
    const count=value=>value.toLocaleString(undefined,{maximumFractionDigits:0});
    const section=title=>{const block=el('section','sdk-usage-group');block.append(el('h4','',title));detail.append(block);return block;};
    const metric=(holder,label,value)=>{const row=el('div','sdk-usage-metric');row.append(el('span','',label),el('b','',value));holder.append(row);};
    const note=(holder,value)=>holder.append(el('p','sdk-control-note',value));
    detail.replaceChildren();
    const tokenValues=[total,input,output,t.reasoning,t.thinking_tokens,t.thinkingTokens,t.reasoningTokens,t.cache?.read,t.cache_read_tokens,t.cacheReadInputTokens,t.cachedInputTokens,t.cachedReadTokens,t.cache?.write,t.cache_creation_input_tokens,t.cacheCreationInputTokens,t.cacheCreationTokens];
    const hasTokens=tokenValues.some(value=>typeof value==='number' && Number.isFinite(value));
    const tokens=hasTokens ? section('累计 Token') : el('section');
    if(total!=null)tokens.append(el('div','sdk-usage-total',count(total)));
    const grid=el('div','sdk-usage-metrics');tokens.append(grid);
    for(const [label,value] of [['输入',input],['输出',output],['思考',t.reasoning ?? t.thinking_tokens ?? t.thinkingTokens ?? t.reasoningTokens],['缓存读取',t.cache?.read ?? t.cache_read_tokens ?? t.cacheReadInputTokens ?? t.cachedInputTokens ?? t.cachedReadTokens],['缓存写入',t.cache?.write ?? t.cache_creation_input_tokens ?? t.cacheCreationInputTokens ?? t.cacheCreationTokens]])
      if(typeof value==='number' && Number.isFinite(value))metric(grid,label,count(value));
    const cost=usage.cost_usd ?? usage.total_cost_usd ?? (Number.isFinite(usage.costUsdTicks)?usage.costUsdTicks/1e10:undefined);
    if(typeof cost==='number' && !usage.usageIsIncomplete && !usage.costIsPartial)metric(grid,'成本',`$${cost.toLocaleString(undefined,{maximumFractionDigits:6})}`);
    if(hasTokens && usage.basis)note(tokens,usage.basis==='native cumulative modelUsage; estimated cost; thinking included in output'?'原生累计用量；成本为估算；思考计入输出':usage.basis);
    const context=usage.context || view.raw?.context_usage || view.usageData?.context_usage;
    if(context && Number.isFinite(context.percent ?? context.percentage)){
      const holder=section('上下文'),percent=context.percent ?? context.percentage;
      holder.append(UsageUI.gauge('已用',percent,context.basis || '原生估计'));
      if(Number.isFinite(context.used_tokens) && Number.isFinite(context.max_tokens))note(holder,`${count(context.used_tokens)} / ${count(context.max_tokens)} Token`);
    }
    if(view.raw?.step_usage){const step=section('最近步骤 · 单独统计');for(const [key,value] of Object.entries(view.raw.step_usage))metric(step,({input_tokens:'输入',output_tokens:'输出',thinking_tokens:'思考',cache_read_tokens:'缓存读取',total_tokens:'Token'})[key] || key,typeof value==='number'?count(value):JSON.stringify(value));}
    if(view.raw?.settings?.fast_mode_state){const state=view.raw.settings.fast_mode_state;note(detail,`Fast 状态：${{off:'关闭',on:'开启',enabled:'开启',disabled:'关闭'}[state] || (typeof state==='string'?state:JSON.stringify(state))}`);}
    if(view.raw?.settings?.fast_mode_disabled_reason)note(detail,`Fast 不可用：${view.raw.settings.fast_mode_disabled_reason}`);
    for(const error of [view.usageData?.quota?.error,view.usageError,...Object.values(view.raw?.usage_errors || {})])if(error){
      const message=String(error),line=el('p','sdk-control-note',message.length>200 ? `${message.slice(0,200)}…` : message);line.title=message;detail.append(line);
    }
  },
  paintSettings(view) {
    const holder=view.pane.querySelector('#native-sdk-settings'),raw=view.raw || {},source=view.uid.split(':')[0];
    const modes=raw.modes?.availableModes || raw.modes || [];
    const definitions=[];
    if(view.capabilities.mode || (view.capabilities.settings && Array.isArray(modes) && modes.length))definitions.push(['mode','模式',Array.isArray(modes)?modes:[],raw.mode || raw.modes?.currentModeId || raw.settings?.mode]);
    if(view.capabilities.permission_mode)definitions.push(['permission_mode','权限模式',['default','acceptEdits','plan','dontAsk','auto','bypassPermissions'],raw.settings?.permission_mode]);
    if(view.capabilities.fast_mode)definitions.push(['fast_mode','Fast',['false','true'],raw.settings?.fast_mode==null?'':String(raw.settings.fast_mode)]);
    if(view.capabilities.settings && source==='opencode')definitions.push(['native_agent','代理',view.nativeAgents || raw.agents || [],raw.agent]);
    for(const definition of definitions){definition[3]=sdkSettingValue(view,definition[0],definition[3] || '');definition[4]=view.settingChoices.has(definition[0]);}
    const signature=JSON.stringify(definitions);if(holder.dataset.state===signature){sdkSyncSettings(view);return;}holder.dataset.state=signature;
    view.pane.querySelector('.sdk-status-row').dataset.footerLayout='';
    for(const control of view.nativeSettings || [])control.remove();view.nativeSettings=[];holder.replaceChildren();
    for(const [key,title,options,current,pending] of definitions) {
      const label=el('label','new-effort sdk-native-setting'),select=document.createElement('select');label.title=title;select.id=`native-sdk-${key}`;select.setAttribute('aria-label',title);
      const names=key==='fast_mode'?{false:'Fast 关闭',true:'Fast 开启'}:{default:key==='permission_mode'?'默认权限':'默认模式',acceptEdits:'允许编辑','accept-edits':'允许编辑',plan:'计划模式',auto:'自动模式',dontAsk:'不询问',bypassPermissions:'跳过审批',build:'构建',general:'通用',explore:'探索'};
      for(const option of options){const id=typeof option==='string'?option:option.id || option.name;select.add(new Option(names[id] || (typeof option==='string'?option:option.name || option.label || id),id));}
      if(current && ![...select.options].some(option=>option.value===current))select.add(new Option(names[current] || current,current));
      ConversationUI.selectValue(select,current || '',`选择${title}`);select.title=`${title}：${select.selectedOptions[0]?.textContent || '未返回'}`;
      select.disabled=pending;
      label.hidden=!options.length && !current;label.append(el('span','visually-hidden',title),select);holder.append(label);view.nativeSettings.push(label);
      select.onchange=async()=>{
        const generation=view.generation,value=select.value;
        try{
          await this.settings(view,{[key]:key==='fast_mode'?value==='true':value});
        }catch(error){if(view.current(generation))view.setNotice(`设置失败：${error.message}`,'blocked');}
      };
    }
    sdkSyncSettings(view);
  },
  async settings(view, values) {
    const generation=view.generation,choices=Object.fromEntries(Object.entries(values).map(([key,value])=>[key,String(value)]));
    await sdkUpdateSettings(view,choices,()=>view.request('api/native-sdk/settings',{
      uid:view.uid,...values,request_id:crypto.randomUUID(),
    }),()=>{view.paintPickers();this.paintSettings(view);this.layout(view);});
    if(view.current(generation))view.setNotice(view.uid.startsWith('agy:')?'设置已保存；当前运行回合保持原设置':'设置已交给原生确认');
  },
  paintQueue(view) {
    const holder=view.pane.querySelector('.sdk-queue');
    if(holder._nativeQueueBusy)return;
    const nativeRows=view.raw?.queue || [],bridgeRows=view.raw?.input_queue || [],steers=view.raw?.steers || [];
    const signature=JSON.stringify([nativeRows,bridgeRows,steers]);if(holder.dataset.state===signature)return;holder.dataset.state=signature;holder.replaceChildren();
    const generation=view.generation,uid=view.uid;
    const labels={preparing:'准备输入',queued:'排队中',sending:'正在发送…',accepted:'原生已接收',failed:'发送失败',unknown:'处理结果待核对'};
    const unknownReason=item=>item.observed_consumed || item.consumption==='observed'
      ? '原生已读取这条输入，但执行结果尚未确认。'
      : '尚未确认原生是否收到或处理了这条输入。';
    const cardFor=(item,title)=>{const card=el('div','msg sdk-request'),body=el('div','mb');card.dataset.queueId=item.id;body.append(el('p','sdk-request-title',title),el('pre','',item.text || item.prompt?.text || item.id));card.append(body);holder.append(card);return body;};
    // Admission/delivery progress belongs to the user's message, like Codex's
    // queue. It is not an assistant request and does not prove consumption.
    const userCardFor=(item,title)=>{
      const {card,body}=ConversationUI.queuedMessage(item.text || item.prompt?.text || item.id,title,
        {uid,agent:view.agent,className:'sdk-queued'});
      card.dataset.queueId=item.id;holder.append(card);return body;
    };
    const actionsFor=body=>{let actions=body.querySelector('.sdk-request-actions');if(!actions){actions=el('div','sdk-request-actions');body.append(actions);}return actions;};
    const operation=(body,item,method,title,extra={})=>{
      const queued=body.parentElement.classList.contains('sdk-queued');
      const button=el('button',queued?'queued-send-dismiss':'btn',title);button.type='button';
      (queued?body.parentElement.querySelector('.sdk-queue-actions'):actionsFor(body)).append(button);
      button.onclick=async()=>{
        if(method==='queue_retry' && item.state==='unknown' && !await appConfirm(`重新发送这条输入？\n\n${unknownReason(item)}重新发送会再次提交整条输入，可能重复执行。请先核对会话历史。`,{ok:'重新发送'}))return;
        if(method==='queue_cancel' && item.state==='unknown' && !await appConfirm('请先核对会话历史。移除此记录不会停止原生任务，后续排队输入将继续。',{ok:'移除记录'}))return;
        if(!view.current(generation))return;
        holder._nativeQueueBusy=true;
        holder.querySelectorAll('button').forEach(control=>control.disabled=true);
        try{await view.request(`api/native-sdk/${method}`,{uid,id:item.id,...extra,...(item.state==='unknown'?{confirmed:true}:{}),request_id:crypto.randomUUID()});}
        catch(error){if(view.current(generation))view.setNotice(error.message,'blocked');}
        finally{holder._nativeQueueBusy=false;if(view.current(generation)){delete holder.dataset.state;this.paintQueue(view);}}
      };
    };
    for(const item of bridgeRows){
      if(item.state==='accepted'){
        const receipt=el('div','native-message-state',labels.accepted);
        receipt.dataset.queueId=item.id;receipt.dataset.queueOrigin='bridge';
        receipt.title='仅表示原生已接收输入，不表示已处理或已完成';
        holder.append(receipt);continue;
      }
      const ordinaryInput=item.operation!=='steer' && ['preparing','queued','sending'].includes(item.state);
      const body=(ordinaryInput?userCardFor:cardFor)(item,labels[item.state] || item.state);body.parentElement.dataset.queueOrigin='bridge';
      if(item.error)body.append(el('p','sdk-control-note',item.error));
      if(item.state==='unknown')body.append(el('p','sdk-control-note',item.operation==='steer'?'引导处理结果尚未确认。请核对历史；移除记录不会停止原生任务。':`${unknownReason(item)}后续输入已暂停。核对历史后，可移除记录以继续，或重新发送这条输入。`));
      if(['failed','unknown'].includes(item.state) && item.operation!=='steer')operation(body,item,'queue_retry',item.state==='unknown'?'重新发送':'重试');
      if(['queued','failed','unknown'].includes(item.state))operation(body,item,'queue_cancel',item.state==='unknown'?'移除记录':'撤销');
      if(['failed','unknown'].includes(item.state) && item.text){const restore=el('button','btn','恢复文字');restore.type='button';restore.onclick=()=>{if(!view.current(generation))return;view.input.value=[view.input.value,item.text].filter(Boolean).join('\n');view.fitInput();CodexSdkCompose.input(view);view.input.focus();view.setNotice('文字已追加到输入框；队列记录保留');};actionsFor(body).append(restore);}
    }
    for(const item of nativeRows){const body=userCardFor(item,item.delivery==='steer'?'等待引导':'排队中');body.parentElement.dataset.queueOrigin='native';
      for(const [method,title,extra] of [['queue_cancel','撤销',{}],['queue_update','立即引导',{delivery:'steer'}]])if(view.capabilities[method] && !(method==='queue_update' && item.delivery==='steer'))operation(body,item,method,title,extra);
    }
    const steerLabels={dispatching:'正在提交引导',native_queued:'原生已接收引导',native_received:'原生已登记引导',rejected:'引导被拒绝',uncertain:'引导结果待核对',connection_closed:'引导连接已关闭'};
    for(const item of steers){const body=cardFor({...item,id:item.id || item.request_id},steerLabels[item.status] || '引导回执');body.parentElement.dataset.queueOrigin='steer';body.append(el('p','sdk-control-note','原生接收回执不能确认输入已被处理；临近任务结束时可能另开一轮。'));if(item.error)body.append(el('p','sdk-control-note',item.error));}
  },
  async action(view,method,body={}) {
    const generation=view.generation;
    if(view.capabilities[method]!==true && !(method==='permissions' && view.uid.startsWith('opencode:') && view.capabilities.settings)){view.setNotice('当前原生接口未提供此操作','blocked');return false;}
    try{
      if(method==='steer'){
        const modal=CodexSdkActions.form('引导当前任务','原生会在可插入时处理；临近任务结束时可能另开一轮。普通发送仍进入队列。','发送引导');
        const input=document.createElement('textarea');input.setAttribute('aria-label','引导内容');modal.fields.append(input);
        modal.form.onsubmit=event=>{event.preventDefault();if(input.value.trim())modal.finish({text:input.value});};body=await modal.answer;if(!body)return false;
      }
      if(method==='permissions'){
        const modal=CodexSdkActions.form('权限规则','编辑 OpenCode 原生 permissions；保存后由原生服务校验。','保存');const input=document.createElement('textarea');input.setAttribute('aria-label','权限规则 JSON');input.value=JSON.stringify(view.raw?.permissions || [],null,2);modal.fields.append(input);
        modal.form.onsubmit=event=>{event.preventDefault();try{modal.finish(JSON.parse(input.value));}catch{modal.error.textContent='请输入有效 JSON';}};
        const permissions=await modal.answer;if(permissions==null)return false;body={permissions};method='settings';
      }
      if(method==='command'){
        const response=await view.request('api/native-sdk/commands',{uid:view.uid});const rows=response.commands || response.raw?.commands || response.raw?.data || response.raw || [];
        const modal=CodexSdkActions.form('原生命令','选择原生服务提供的命令。','运行');const select=document.createElement('select');select.setAttribute('aria-label','原生命令');for(const row of Array.isArray(rows)?rows:[])select.add(new Option(row.name,row.name));const input=document.createElement('textarea');input.setAttribute('aria-label','命令参数');modal.fields.append(select,input);modal.form.onsubmit=event=>{event.preventDefault();if(select.value)modal.finish({name:select.value,text:input.value});};body=await modal.answer;if(!body)return false;
      }
      if(method==='shell' && !body.command){const modal=CodexSdkActions.form('运行命令','由原生接口直接运行命令，使用该来源的实际权限策略。','运行');const input=document.createElement('textarea');input.setAttribute('aria-label','命令');modal.fields.append(input);modal.form.onsubmit=event=>{event.preventDefault();if(input.value.trim())modal.finish(input.value);};body.command=await modal.answer;if(!body.command)return false;}
      if(method==='revert' && !body.message_id){const modal=CodexSdkActions.form('回退对话','选择要回退到的消息之前；不回滚工作区文件。','回退');const select=document.createElement('select');select.setAttribute('aria-label','回退消息');for(const item of view.items.filter(item=>item.role==='user'))select.add(new Option((item.text || item.id).slice(0,100),item.id));modal.fields.append(select);modal.form.onsubmit=event=>{event.preventDefault();if(select.value)modal.finish(select.value);};body.message_id=await modal.answer;if(!body.message_id)return false;}
      if(method==='rewind' && body.prompt_index==null){const response=await view.request('api/native-sdk/rewind_points',{uid:view.uid});const points=response.rewind_points || response.raw?.rewind_points || response.raw?.points || [];const modal=CodexSdkActions.form('回退对话','仅回退会话历史，不回滚工作区文件。','回退');const select=document.createElement('select');select.setAttribute('aria-label','回退消息');for(const point of points)select.add(new Option(point.prompt_preview || point.preview || String(point.prompt_index),point.prompt_index));modal.fields.append(select);modal.form.onsubmit=event=>{event.preventDefault();if(select.value!=='')modal.finish(Number(select.value));};body.prompt_index=await modal.answer;if(body.prompt_index==null)return false;}
      if(method==='revert_commit' && !await appConfirm('提交回退会永久移除被回退的对话记录；工作区文件不会回滚。',{ok:'提交回退'}))return false;
      if(!view.current(generation))return false;
      const response=await view.request(`api/native-sdk/${method}`,{uid:view.uid,...body,request_id:crypto.randomUUID()});
      if(!view.current(generation))return true;
      if((response.accepted ?? response.raw?.accepted)===false)throw new Error(response.error || response.raw?.error || '原生未确认接收，请核对操作记录');
      if(method==='fork'){await CodexSdkActions.openFork(view,response);return true;}
      if(['revert','unrevert','revert_commit','rewind'].includes(method))await view.refreshHistory(generation,true);
      if(method==='compact')view.paintStatus();else view.setNotice('原生操作已接受');return true;
    }catch(error){if(view.current(generation))view.setNotice(`操作失败：${error.message}`,'blocked');return false;}
  },
  async command(view,text) {
    const match=/^\/(\S+)(?:\s+([\s\S]*))?$/.exec(text.trim());if(!match && !text.trim().startsWith('!'))return false;
    const command=match?.[1] || 'shell',arg=(match?.[2] || (match?'':text.trim().slice(1))).trim();
    let success=false;
    if(['usage','status','help'].includes(command)){
      if(command==='usage')void this.loadUsage(view,true);
      if(command==='help')view.setNotice('网页命令：/usage、/status、/model ID、/effort 档位、/stop、/fork、/compact、/revert、/shell 命令。会话操作按原生能力开放。');
      else if(command==='status'){const menu=view.pane.querySelector('#sdk-status-menu');menu.showPopover();ConversationUI.placePopover(menu,view.notice);}
      else {
        const menu=view.pane.querySelector('#sdk-more-menu'),button=view.pane.querySelector('#sdk-more');
        if(!button.hidden && !menu.matches(':popover-open')){menu.showPopover();ConversationUI.placePopover(menu,button);}
      }success=true;
    }else if(command==='stop'){await view.interrupt();success=true;}
    else if(['model','effort'].includes(command)){
      if(!arg)view.setNotice(`用法：/${command} 值`,'blocked');
      else {await this.settings(view,{[command]:arg});success=true;}
    }else if(['fork','compact','revert','shell'].includes(command))success=await this.action(view,command,command==='shell'?{command:arg}:command==='fork'&&arg?{before:arg}:{});
    else view.setNotice(`此网页尚未映射 /${command}，输入已保留。普通文字请去掉开头的 /。`,'blocked');
    if(success && view.input?.value===text){view.input.value='';view.fitInput();CodexSdkCompose.input(view);}
    return true;
  },
  decorate(view,node,item) {
    if(item.role!=='user' || !view.capabilities.fork || view.uid.startsWith('grok:') && item.prompt_index==null)return;
    const toggle=el('button','sdk-bubble-toggle','⋯');toggle.type='button';toggle.setAttribute('aria-label','消息操作');toggle.setAttribute('aria-expanded','false');
    const menu=el('div','new-model-menu sdk-bubble-menu');menu.setAttribute('popover','auto');menu.setAttribute('aria-label','消息操作');
    const button=el('button','btn','从此处之前分支');button.type='button';button.onclick=()=>{menu.hidePopover();this.action(view,'fork',view.uid.startsWith('grok:')?{prompt_index:item.prompt_index}:{before:item.native_id || item.id});};menu.append(button);
    toggle.onclick=()=>{if(menu.matches(':popover-open'))menu.hidePopover();else{menu.showPopover();ConversationUI.placePopover(menu,toggle);}};menu.addEventListener('beforetoggle',event=>toggle.setAttribute('aria-expanded',String(event.newState==='open')));node.classList.add('sdk-user-actions');node.append(toggle,menu);
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
