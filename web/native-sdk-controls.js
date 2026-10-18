'use strict';

// Native capabilities share presentation, never another provider's wire semantics.
const NativeSdkControls = {
  mount(view) {
    const row = view.pane.querySelector('.sdk-status-row');
    const gauges = el('div','sdk-gauges'); gauges.setAttribute('aria-label','用量');
    const usage = el('button','btn','用量'); usage.id = 'sdk-usage-toggle'; usage.type = 'button';
    const more = el('button','btn','⋯'); more.id = 'sdk-more'; more.type = 'button'; more.setAttribute('aria-label','会话操作');
    const menu = el('div','new-model-menu sdk-toolbar-menu'); menu.id = 'sdk-more-menu'; menu.setAttribute('popover','auto'); menu.setAttribute('aria-label','会话操作');
    const usageMenu = el('div','new-model-menu sdk-toolbar-menu sdk-usage-menu'); usageMenu.id = 'sdk-usage-menu'; usageMenu.setAttribute('popover','auto'); usageMenu.setAttribute('aria-label','用量');
    const detail = el('div','sdk-control-section'); detail.id = 'native-usage-detail';
    const refresh = el('button','btn','刷新用量'); refresh.type = 'button'; refresh.id = 'native-usage-refresh';
    refresh.onclick = () => this.loadUsage(view,true);
    usageMenu.append(detail,refresh);
    row.insertBefore(gauges,row.querySelector('.sdk-options'));
    row.insertBefore(usage,row.querySelector('.sdk-options'));
    view.pane.querySelector('.sdk-pickers').append(more,menu,usageMenu);
    for (const [button,popover] of [[more,menu],[usage,usageMenu]]) {
      button.setAttribute('aria-expanded','false'); button.setAttribute('aria-controls',popover.id);
      popover.addEventListener('beforetoggle',event => button.setAttribute('aria-expanded',String(event.newState === 'open')));
      button.onclick = () => { if (popover.matches(':popover-open')) popover.hidePopover(); else {popover.showPopover(); CodexSdkView.placeMenu(popover,button);} };
    }
    const actions = el('div',''); actions.id = 'sdk-actions'; menu.append(actions);
    for (const [method,label] of [['fork','分支会话'],['compact','压缩上下文'],['revert','回退对话'],['unrevert','撤销回退'],['revert_commit','提交回退'],['rewind','回退对话'],['shell','运行命令'],['command','原生命令'],['permissions','权限规则']]) {
      const button = el('button','btn sdk-action',label); button.type = 'button';button.dataset.sdkAction = method;
      button.onclick = () => { view.closeToolbarMenus(); void this.action(view,method); }; actions.append(button);
    }
    const settings = el('div','sdk-control-section'); settings.id = 'native-sdk-settings'; menu.append(settings);
    view.nativeResize = new ResizeObserver(() => this.layout(view)); view.nativeResize.observe(row);
    this.paint(view);
  },
  close(view) { view.nativeResize?.disconnect(); view.nativeResize = null; CodexSdkActions.close(); },
  layout(view) {
    if (!view.pane) return;
    const row=view.pane.querySelector('.sdk-status-row'),gauges=view.pane.querySelector('.sdk-gauges'),menu=view.pane.querySelector('#sdk-usage-menu');
    if (!gauges || !menu) return;
    // A stable breakpoint avoids oscillation when moving gauges changes intrinsic width.
    const compact=row.clientWidth < 760;
    if (compact) { if (gauges.parentElement !== menu) menu.prepend(gauges); }
    else if (gauges.parentElement !== row) row.insertBefore(gauges,row.querySelector('#sdk-usage-toggle'));
    const effort=view.pane.querySelector('.sdk-effort'),more=view.pane.querySelector('#sdk-more-menu');
    if (row.clientWidth < 400) { if (effort.parentElement !== more) more.prepend(effort); }
    else if (effort.parentElement !== view.pane.querySelector('.sdk-pickers')) view.pane.querySelector('.sdk-pickers').insertBefore(effort,view.pane.querySelector('#sdk-more'));
  },
  async loadUsage(view,refresh) {
    const generation=view.generation;
    if (!view.uid || view.loadingUsage) return;
    view.loadingUsage=true;
    const button=view.pane?.querySelector('#native-usage-refresh'); if (button) button.disabled=true;
    try {
      const response=await view.request('api/native-sdk/usage',{uid:view.uid,refresh});
      if (!view.current(generation)) return;
      view.usageData={...response,...response.raw};view.usageError=null;
      if (view.capabilities.billing) {
        try { const billing=await view.request('api/native-sdk/billing',{uid:view.uid}); if (view.current(generation)) view.usageData.billing=billing.raw?.billing || billing.raw; }
        catch (error) { if (view.current(generation)) view.usageError=`额度：${error.message}`; }
      }
    } catch (error) { if (view.current(generation)) view.usageError=error.message; }
    finally { if (view.current(generation)) {view.loadingUsage=false;if(button)button.disabled=false;this.paintUsage(view);} }
  },
  paint(view) {
    if (!view.pane) return;
    for (const button of view.pane.querySelectorAll('[data-sdk-action]')) button.hidden=view.capabilities[button.dataset.sdkAction] !== true && !(button.dataset.sdkAction==='permissions' && view.uid.startsWith('opencode:') && view.capabilities.settings);
    let mediaNote=view.pane.querySelector('#native-media-note');
    if(!mediaNote){mediaNote=el('p','sdk-control-note');mediaNote.id='native-media-note';view.pane.querySelector('#sdk-composer').append(mediaNote);}
    mediaNote.hidden=!(view.uid.startsWith('agy:') || view.uid.startsWith('grok:') && view.capabilities.images===false);
    mediaNote.textContent=mediaNote.hidden?'':'此原生接口仅发送图片路径引用，不传递图像内容。';
    this.paintSettings(view);this.paintUsage(view);this.paintQueue(view);this.layout(view);
  },
  paintUsage(view) {
    if (!view.pane) return;
    const gauges=view.pane.querySelector('.sdk-gauges'),detail=view.pane.querySelector('#native-usage-detail');if(!gauges || !detail)return;
    gauges.replaceChildren();detail.replaceChildren();
    const usage=view.raw?.usage || view.usageData?.usage || {},t=usage.tokens || usage;
    const number=(...values)=>values.find(value=>typeof value==='number' && Number.isFinite(value));
    const count=value=>value.toLocaleString(undefined,{maximumFractionDigits:0});
    const input=number(t.input,t.input_tokens,t.inputTokens),output=number(t.output,t.output_tokens,t.outputTokens);
    const total=number(t.total_tokens,t.totalTokens,input != null && output != null ? input+output : null);
    const text=(label,value)=>{const line=el('div','sdk-control-note',`${label}：${value}`);detail.append(line);};
    const gauge=(label,used,title)=>{
      if (!Number.isFinite(used)) return;
      const node=el('span','sdk-gauge');node.title=title;
      node.append(el('span','',label));const bar=el('i','sdk-bar'),fill=el('u','');fill.style.width=`${Math.max(0,Math.min(100,used))}%`;bar.setAttribute('role','progressbar');bar.setAttribute('aria-label',`${label} 已用`);bar.setAttribute('aria-valuemin','0');bar.setAttribute('aria-valuemax','100');bar.setAttribute('aria-valuenow',String(Math.max(0,Math.min(100,used))));bar.append(fill);node.append(bar,el('b','',`${Math.round(used)}%`));node.classList.toggle('low',used>=90);gauges.append(node);text(label,title);
    };
    if(total!=null){const node=el('span','sdk-credits',`${count(total)} tokens`);node.title='累计用量；不是当前上下文占用';gauges.append(node);}
    for(const [label,value] of [['输入 Token',input],['输出 Token',output],['思考 Token',number(t.reasoning,t.thinking_tokens,t.thinkingTokens,t.reasoningTokens)],['缓存读取',number(t.cache?.read,t.cache_read_tokens,t.cacheReadInputTokens,t.cachedInputTokens,t.cachedReadTokens)],['缓存写入',number(t.cache?.write,t.cache_creation_input_tokens,t.cacheCreationInputTokens,t.cacheCreationTokens)],['累计 Token',total]])if(value!=null)text(label,count(value));
    const cost=usage.usageIsIncomplete || usage.costIsPartial ? undefined : number(usage.cost_usd,usage.total_cost_usd,typeof usage.costUsdTicks==='number'?usage.costUsdTicks/1e10:null); if(cost!=null)text('成本',`$${cost.toLocaleString(undefined,{maximumFractionDigits:6})}`);
    const context=usage.context || view.raw?.context_usage || view.usageData?.context_usage || {};
    const percent=number(context.percent,context.percentage);if(percent!=null)gauge('上下文',percent,`已用 ${percent.toFixed(1)}%${context.used_tokens!=null ? `（${count(context.used_tokens)} / ${count(context.max_tokens)} tokens）`:''}；${context.basis || '原生估计'}`);
    const quota=view.usageData?.quota;
    for(const bucket of quota?.buckets || [])gauge(bucket.window || bucket.group || '额度',typeof bucket.remaining_percent==='number'?100-bucket.remaining_percent:NaN,`${bucket.group || bucket.id} 剩余 ${bucket.remaining_percent}%${bucket.resets_at ? `；${new Date(bucket.resets_at).toLocaleString()} 重置`:''}${quota.captured_at ? `；记录于 ${new Date(quota.captured_at).toLocaleString()}`:''}`);
    if(quota?.error)text('额度查询',quota.error);
    const billing=view.raw?.billing || view.usageData?.billing;const b=billing?.config || billing;
    if(b){gauge('额度',b.creditUsagePercent,`本周期已用 ${b.creditUsagePercent}%${b.currentPeriod?.end ? `；${new Date(b.currentPeriod.end).toLocaleString()} 重置`:''}`);if(b.prepaidBalance?.val!=null)text('预付余额',`$${(b.prepaidBalance.val/100).toFixed(2)}`);}
    const rate=view.raw?.rate_limits || view.usageData?.rate_limits;
    if(rate && Object.keys(rate).length) {const line=el('pre','sdk-control-note',JSON.stringify(rate,null,2));detail.append(el('div','sdk-control-note','原生限额'),line);}
    if(usage.basis)text('统计范围',usage.basis);
    if(view.raw?.step_usage)text('最近步骤用量',JSON.stringify(view.raw.step_usage));
    for(const [key,error] of Object.entries(view.raw?.usage_errors || {}))text(`用量 ${key}`,error);
    if(view.raw?.settings?.fast_mode_state)text('Fast 状态',JSON.stringify(view.raw.settings.fast_mode_state));
    if(view.raw?.settings?.fast_mode_disabled_reason)text('Fast 不可用原因',view.raw.settings.fast_mode_disabled_reason);
    if(view.usageError)text('查询未完成',view.usageError);
    if(!detail.childElementCount)text('用量','原生尚未提供用量数据');
    this.layout(view);
  },
  paintSettings(view) {
    const holder=view.pane.querySelector('#native-sdk-settings'),raw=view.raw || {},source=view.uid.split(':')[0];
    const modes=raw.modes?.availableModes || raw.modes || [];
    const definitions=[];
    if(view.capabilities.mode || (view.capabilities.settings && Array.isArray(modes) && modes.length))definitions.push(['mode','模式',Array.isArray(modes)?modes:[],raw.mode || raw.settings?.mode]);
    if(view.capabilities.permission_mode)definitions.push(['permission_mode','权限模式',['default','acceptEdits','plan','dontAsk','auto','bypassPermissions'],raw.settings?.permission_mode]);
    if(view.capabilities.fast_mode)definitions.push(['fast_mode','Fast',['false','true'],raw.settings?.fast_mode==null?'':String(raw.settings.fast_mode)]);
    if(view.capabilities.settings && source==='opencode')definitions.push(['native_agent','代理',view.nativeAgents || raw.agents || [],raw.agent]);
    const signature=JSON.stringify(definitions);if(holder.dataset.state===signature)return;holder.dataset.state=signature;holder.replaceChildren();
    for(const [key,title,options,current] of definitions) {
      const label=el('label','new-effort',title+' '),select=document.createElement('select');select.id=`native-sdk-${key}`;select.setAttribute('aria-label',title);
      select.add(new Option(current || '原生默认',''));
      for(const option of options){const id=typeof option==='string'?option:option.id || option.name;select.add(new Option(typeof option==='string'?option:option.name || option.label || id,id));}
      select.value=current || '';label.append(select);holder.append(label);
      select.onchange=async()=>{const generation=view.generation;select.disabled=true;try{await view.request('api/native-sdk/settings',{uid:view.uid,[key]:key==='fast_mode'?select.value==='true':select.value,request_id:crypto.randomUUID()});if(view.current(generation))view.setNotice(source==='agy'?'设置将在下一轮生效':'设置已交给原生确认');}catch(error){if(view.current(generation))view.setNotice(`设置失败：${error.message}`,'blocked');}finally{if(view.current(generation)){select.disabled=false;holder.dataset.state='';this.paintSettings(view);}}};
    }
  },
  paintQueue(view) {
    const holder=view.pane.querySelector('.sdk-queue'),rows=view.raw?.queue;
    if(!Array.isArray(rows))return;
    const signature=JSON.stringify(rows);if(holder.dataset.state===signature)return;holder.dataset.state=signature;holder.replaceChildren();
    for(const item of rows){const card=el('div','msg sdk-request'),body=el('div','mb');card.dataset.queueId=item.id;body.append(el('p','sdk-request-title',item.delivery==='steer'?'等待引导':'排队中'),el('pre','',item.text || item.prompt?.text || item.id));
      for(const [method,title,extra] of [['queue_cancel','撤销',{}],['queue_update','立即引导',{delivery:'steer'}]]){if(!view.capabilities[method])continue;const button=el('button','btn',title);button.type='button';button.onclick=async()=>{button.disabled=true;try{await view.request(`api/native-sdk/${method}`,{uid:view.uid,id:item.id,...extra,request_id:crypto.randomUUID()});}catch(error){view.setNotice(error.message,'blocked');button.disabled=false;}};body.append(button);}card.append(body);holder.append(card);}
  },
  async action(view,method,body={}) {
    const generation=view.generation;
    if(view.capabilities[method]!==true && !(method==='permissions' && view.uid.startsWith('opencode:') && view.capabilities.settings)){view.setNotice('当前原生接口未提供此操作','blocked');return false;}
    try{
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
      if(method==='fork'){await CodexSdkActions.openFork(view,response);return true;}
      if(['revert','unrevert','revert_commit','rewind'].includes(method))await view.refreshHistory(generation,true);
      view.setNotice('原生操作已接受');return true;
    }catch(error){if(view.current(generation))view.setNotice(`操作失败：${error.message}`,'blocked');return false;}
  },
  async command(view,text) {
    const match=/^\/(\S+)(?:\s+([\s\S]*))?$/.exec(text.trim());if(!match && !text.trim().startsWith('!'))return false;
    const command=match?.[1] || 'shell',arg=(match?.[2] || (match?'':text.trim().slice(1))).trim();
    let success=false;
    if(['usage','status','help'].includes(command)){
      if(command==='usage')await this.loadUsage(view,true);
      if(command==='help')view.setNotice('网页命令：/usage、/status、/model ID、/effort 档位、/stop、/fork、/compact、/revert、/shell 命令。会话操作按原生能力开放。');
      else{const button=view.pane.querySelector(command==='usage'?'#sdk-usage-toggle':'.sdk-status');button.click();}success=true;
    }else if(command==='stop'){await view.interrupt();success=true;}
    else if(['model','effort'].includes(command)){
      if(!arg)view.setNotice(`用法：/${command} 值`,'blocked');
      else {view[command==='model'?'modelChoice':'effortChoice']=arg;view.paintPickers();view.setNotice('将在下一次发送时应用');success=true;}
    }else if(['fork','compact','revert','shell'].includes(command))success=await this.action(view,command,command==='shell'?{command:arg}:command==='fork'&&arg?{before:arg}:{});
    else view.setNotice(`此网页尚未映射 /${command}，输入已保留。普通文字请去掉开头的 /。`,'blocked');
    if(success && view.input?.value===text){view.input.value='';view.fitInput();CodexSdkCompose.input(view);}
    return true;
  },
  decorate(view,node,item) {
    if(item.role!=='user' || !view.capabilities.fork || view.uid.startsWith('grok:') && item.prompt_index==null)return;
    const toggle=el('button','sdk-bubble-toggle','⋯');toggle.type='button';toggle.setAttribute('aria-label','消息操作');toggle.setAttribute('aria-expanded','false');
    const menu=el('div','new-model-menu sdk-bubble-menu');menu.setAttribute('popover','auto');menu.setAttribute('aria-label','消息操作');
    const button=el('button','btn','从此处之前分支');button.type='button';button.onclick=()=>{menu.hidePopover();this.action(view,'fork',view.uid.startsWith('grok:')?{prompt_index:item.prompt_index}:{before:item.id});};menu.append(button);
    toggle.onclick=()=>{if(menu.matches(':popover-open'))menu.hidePopover();else{menu.showPopover();CodexSdkView.placeMenu(menu,toggle);}};menu.addEventListener('beforetoggle',event=>toggle.setAttribute('aria-expanded',String(event.newState==='open')));node.classList.add('sdk-user-actions');node.append(toggle,menu);
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
