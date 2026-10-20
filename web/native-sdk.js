'use strict';

// Native providers share the SDK surface, draft/upload store and history
// renderer. Protocol translation and process ownership stay on the node.
const NativeSdkView = {
  ...SdkUI.methods,
  uid: '', agent: null, pane: null, generation: 0, connected: false,
  available(uid, agent) {
    const source = String(uid || '').split(':')[0];
    return !(agent && source === 'claude') && SessionDockCapabilities.config.native_sdks?.[source] === true
      && (!HUB_MODE || Nodes.capabilities[nodeOf(uid)]?.native_sdks?.[source] === true);
  },
  layoutToolbar() { NativeSdkControls.layout(this); },

  open(uid, pane, agent = null, {historyReady = null, readOnly = false} = {}) {
    this.close();
    this.uid = uid; this.agent = agent; this.pane = pane;
    this.historyReady = historyReady;
    this.readOnly = readOnly;
    this.pendingAtOpen = !agent && !!this.pending(uid);
    this.items = []; this.requests = new Map(); this.nodes = new Map();
    this.nativeAgents = null; this.loadingUsage = false; this.raw = {}; this.usageData = {}; this.usageError = null;
    this.models = []; this.capabilities = {}; this.running = false; this.seq = -1;
    this.historyRenderedData = null; this.historyCut = null;
    this.nativeModel = this.nativeEffort = null;
    const generation = ++this.generation;
    const name = SOURCES[String(uid).split(':')[0]]?.name || 'Agent';
    pane.classList.add('native-sdk-pane');
    SdkUI.mount(this, {name, readOnly, historyReady,
      settings: (view, values) => NativeSdkControls.settings(view, values)});
    NativeSdkControls.mount(this);
    this.mountToolbar(generation);
    CodexSdkCompose.mount(this);
    void this.refreshHistory(generation);
    void this.start(generation);
  },

  close() {
    this.setConnected(false);
    CodexSdkCompose.close(this);
    if (this.input) PathCompletion.close(this.input);
    NativeSdkControls.close(this);
    SdkUI.closeToolbar(this);
    this.generation++;
    this.es?.close(); this.es = null;
    this.openRead?.abort(); this.openRead = null;
    clearTimeout(this.reconnectTimer); clearTimeout(this.historyTimer);
    disposeMessageObservers(this.box);
    this.uid = ''; this.agent = null;
    this.pane = this.box = this.history = this.live = this.notice = this.input = null;
    this.historyReady = this.historyData = this.historyEntry = null;
    this.historyFailed = this.openFailed = false;
  },

  async start(generation, reconnecting = false) {
    if (this.readOnly || !this.current(generation) || this.openRead || this.es) return;
    const read = new AbortController(); this.openRead = read;
    try {
      const data = await this.request('api/native-sdk/open', {uid:this.uid}, read.signal);
      if (!this.current(generation)) return;
      this.capabilities = data.capabilities || data.raw?.capabilities || {};
      this.openFailed = false;
      void this.loadModels(generation);
      if (this.capabilities.agents) void this.request('api/native-sdk/agents',{uid:this.uid}).then(response => {if(this.current(generation)){this.nativeAgents=response.agents || response.raw?.agents || response.raw?.data || response.raw;NativeSdkControls.paintSettings(this);}}).catch(()=>{});
      void NativeSdkControls.loadUsage(this, this.uid.startsWith('agy:') ? 'auto' : true);
      const es = new EventSource(appUrl(`api/native-sdk/events?${new URLSearchParams({uid:this.uid,agent:this.agent || ''})}`));
      this.es = es;
      es.addEventListener('sdk', event => {
        if (!this.current(generation) || this.es !== es) return;
        let packet;
        try { packet = JSON.parse(event.data); } catch { return; }
        if (packet.method === 'bridge/stopped') { SdkView.stopped(this.uid, this.agent); return; }
        if (packet.method === 'bridge/reset') { this.reconnect(); return; }
        if (packet.type === 'input_queue') {
          const stick = this.nearBottom();
          this.raw.input_queue = packet.raw?.input_queue || [];
          NativeSdkControls.paintQueue(this);
          this.paintStatus();
          if (stick) this.scrollBottom();
          return;
        }
        if (packet.type === 'snapshot' || packet.method === 'snapshot') {
          this.seq = packet.seq ?? this.seq;
          this.applySnapshot(packet.raw || {});
        }
      });
      es.onerror = () => {
        if (!this.current(generation) || this.es !== es) return;
        this.setConnected(false); this.setNotice('重连中', 'connecting');
        if (es.readyState === EventSource.CLOSED) this.reconnect();
      };
    } catch (error) {
      if (this.current(generation)) {
        this.openFailed = true;
        this.setNotice(`无法打开 SDK 模式：${error.message}`, 'blocked');
        if (reconnecting || error.code === 'native_sdk_session_busy') this.reconnect();
      }
    } finally { if (this.openRead === read) this.openRead = null; }
  },

  retryOpen() { if (this.openFailed) { this.es?.close(); this.es = null; void this.start(this.generation); } },
  reconnect() {
    this.es?.close(); this.es = null; this.seq = -1;
    clearTimeout(this.reconnectTimer);
    const generation = this.generation;
    this.reconnectTimer = setTimeout(() => { if (this.current(generation)) void this.start(generation,true); }, 1000);
  },

  async loadModels(generation) {
    if (this.capabilities.models === false) return;
    try {
      const response = await this.request('api/native-sdk/models', {uid:this.uid});
      if (!this.current(generation)) return;
      const raw = response.raw;
      const options = raw?.configOptions || [];
      const choices = Array.isArray(raw) ? raw : raw?.data || raw?.models?.availableModels || raw?.models
        || options.find(option => option.id === 'model')?.options || [];
      this.models = (Array.isArray(choices) ? choices : []).map(model => typeof model === 'string' ? {id:model} : {
        ...model, id:model.id || model.modelId || model.value || model.model,
        efforts:model.efforts || model.supportedEffortLevels || model.variants || options.find(option => option.id === 'reasoning_effort')?.options?.map(option => option.value),
      }).filter(model => !model.hidden);
      this.paintPickers();
    } catch { /* A failed catalog does not prevent sending with native settings. */ }
  },

  modelChoices() {
    return this.uid.startsWith('agy:') ? agyModelCatalog({models:this.models.map(row => ({
      ...row, name:row.displayName || row.name || row.id,
    }))}).models : this.models;
  },

  paintPickers() {
    if (!this.pane) return;
    const agy = this.uid.startsWith('agy:');
    const models = this.modelChoices();
    const model = this.pane.querySelector('#sdk-model');
    model.replaceChildren();
    for (const row of models) {
      const id = typeof row === 'string' ? row : row.id || row.model || row.modelId;
      if (id && ![...model.options].some(option=>option.value===id)) model.add(new Option((agy ? row.name : row.displayName || row.name) || id, id));
    }
    const modelName=sdkSettingValue(this,'model',this.nativeModel || '');
    // Agy's native settings notices name the model rather than its catalog ID.
    const namedModel=this.uid.startsWith('agy:') ? this.models.find(row=>(row.displayName || row.name)===modelName) : null;
    const modelId=namedModel?.id || modelName;
    const selected = models.find(row => row.id === modelId || agy && row.variants && Object.values(row.variants).includes(modelId));
    const variants = agy ? selected?.variants : null;
    ConversationUI.selectValue(model,selected?.id || modelId,'选择模型');
    if (this.raw?.model_observed && modelName === this.nativeModel && model.selectedOptions[0]) {
      model.selectedOptions[0].textContent += '（上次使用）';
    }
    model.title = model.selectedOptions[0]?.textContent || '模型';
    model.closest('label').hidden = !this.models.length && !modelName;
    const efforts = selected?.supportedReasoningEfforts || selected?.efforts || [];
    const effort = this.pane.querySelector('#sdk-effort');
    effort.replaceChildren();
    for (const row of efforts) { const id = typeof row === 'string' ? row : row.reasoningEffort || row.id; if (id != null && ![...effort.options].some(option=>option.value===id)) effort.add(new Option(id ? UsageUI.effort(id) : 'CLI 默认', id)); }
    let chosen=sdkSettingValue(this,'effort',this.nativeEffort || '');
    if (variants) chosen = Object.entries(variants).find(([,id])=>id===modelId)?.[0] || '';
    if(chosen && ![...effort.options].some(option=>option.value===chosen)){
      if(modelName!==this.nativeModel && !this.settingChoices.has('effort'))chosen='';
      else effort.add(new Option(UsageUI.effort(chosen),chosen));
    }
    if (variants && Object.hasOwn(variants, '')) effort.value = chosen;
    else ConversationUI.selectValue(effort,chosen,'选择强度');
    effort.title = effort.selectedOptions[0]?.textContent || '强度';
    effort.closest('label').hidden = !efforts.length && !chosen;
    NativeSdkControls.layout(this);
  },

  modelSettings(values) {
    if (!this.uid.startsWith('agy:') || !('model' in values || 'effort' in values)) return values;
    const models = this.modelChoices();
    const current = this.models.find(row => (row.displayName || row.name) === this.nativeModel)?.id || this.nativeModel;
    const id = values.model ?? current;
    const selected = models.find(row => row.id === id || row.variants && Object.values(row.variants).includes(id));
    if (!selected?.variants) return values;
    const variant = Object.entries(selected.variants).find(([,native])=>native===id)?.[0];
    const previous = Object.entries(selected.variants).find(([,native])=>native===current)?.[0];
    const level = values.effort ?? variant ?? previous ?? (selected.efforts.includes('high') ? 'high' : selected.efforts[0]);
    if (!Object.hasOwn(selected.variants, level)) throw new Error('该模型不提供此强度');
    // The native ID already encodes effort. Clear any previous independent flag.
    return {...values, model:selected.variants[level], effort:''};
  },

  applySnapshot(raw) {
    const stick = this.nearBottom();
    const completed = this.running && !raw.running;
    const observedModelChanged = this.raw?.model_observed !== raw.model_observed;
    this.raw = raw;
    this.running = !!raw.running;
    const model = typeof raw.model === 'string' ? raw.model : raw.model?.providerID && raw.model?.id
      ? `${raw.model.providerID}/${raw.model.id}` : raw.model?.id || raw.model?.modelID || null;
    const effort = raw.effort || null;
    if (model !== this.nativeModel || effort !== this.nativeEffort || observedModelChanged) {
      this.nativeModel = model; this.nativeEffort = effort; this.paintPickers();
    }
    this.openFailed = !!raw.error;
    this.capabilities = {...this.capabilities, ...raw.capabilities};
    this.items = (raw.items || []).map(nativeSdkMessage);
    this.setConnected(true);
    this.paintHistory();
    const ids = new Set();
    for (const item of this.items) {
      const id = String(item.id); ids.add(id);
      const old = this.nodes.get(id);
      if (old?._updateMessage && old.dataset.role === item.role) { old._updateMessage(item); continue; }
      const toolState = item.role === 'tool' ? ConversationUI.toolState(old) : null;
      const node = msgNode(item); node.dataset.sdkItem = id;
      if (item.role === 'tool' && ['pending','in_progress'].includes(item.status)) {
        // Keep partial stdout rendered, but never label a partial rawOutput as
        // completed just because it already contains a default exit_code:0.
        const status = node.querySelector('.tool-status') || el('div','tool-status');
        status.textContent = item.status === 'pending' ? '等待执行…' : '运行中…';
        if (!status.parentElement) node.querySelector('.tool-entry')?.append(status);
      }
      NativeSdkControls.decorate(this,node,item);
      if (old) old.replaceWith(node); else this.live.append(node);
      ConversationUI.restoreToolState(node, toolState);
      this.nodes.set(id, node);
    }
    for (const [id,node] of this.nodes) if (!ids.has(id)) { node.remove(); this.nodes.delete(id); }
    const requests = new Map((raw.requests || []).map(request => [request.key, request]));
    for (const [key] of this.requests) if (!requests.has(key)) this.live.querySelector(`[data-sdk-request="${CSS.escape(key)}"]`)?.remove();
    for (const [key,request] of requests) if (!this.requests.has(key)) this.live.append(this.requestCard(request));
    this.requests = requests;
    this.pane.querySelector('#sdk-stop').hidden = !this.running || this.capabilities.interrupt === false;
    this.paintStatus();
    NativeSdkControls.paint(this);
    if (completed) { this.scheduleHistory(); void NativeSdkControls.loadUsage(this, false); }
    if (stick) this.scrollBottom();
  },

  paintStatus() {
    const raw=this.raw || {},pending=(raw.input_queue || []).find(item=>item.operation!=='steer');
    const send=this.pane.querySelector('#sdk-send');send.textContent=this.running?'排队':'发送';
    sdkSyncSettings(this);
    send.title=this.running?'当前任务结束后发送；尚未投递的排队消息可撤销':'开始新回合';
    const retryReason=raw.retry?.error?.message || raw.retry?.message || raw.retry?.reason;
    const paused=!this.running && !this.requests.size && ['failed','unknown'].includes(pending?.state);
    const compacting = raw.status === 'compacting' || raw.compact?.state === 'compacting';
    const kind = raw.error ? 'blocked' : this.requests.size ? 'waiting' : compacting ? 'maintenance'
      : raw.retry ? 'retrying' : this.running ? 'working' : paused ? 'warning' : pending ? 'waiting' : 'ready';
    this.setNotice(raw.error ? `错误：${typeof raw.error === 'string' ? raw.error : raw.error.message || JSON.stringify(raw.error)}`
      : this.requests.size ? ([...this.requests.values()].some(request=>request.type==='approval')?'待批准':'待回答') : compacting ? '压缩中' : raw.retry ? `正在重试${raw.retry.attempt ? `（${raw.retry.attempt}）` : ''}${retryReason?`：${retryReason}`:''}` : this.running ? '处理中' : paused ? '队列暂停，请处理失败或待核对记录' : pending ? '等待发送' : '就绪', kind);
  },

  paintHistory() {
    if (!this.history || !this.historyData) return;
    let messages = this.historyData.messages || [];
    let cut = messages.length;
    const first = this.items?.find(item => item.role === 'user');
    if (first) {
      let at = messages.findIndex(message => message.id && message.id === first.id);
      if (at < 0) at = messages.findLastIndex(message => message.role === 'user' && message.text === first.text);
      if (at >= 0) { cut = at; messages = messages.slice(0, at); }
    }
    if (this.historyRenderedData === this.historyData && this.historyCut === cut) return;
    this.historyRenderedData = this.historyData; this.historyCut = cut;
    SdkUI.paintHistory(this, messages);
  },

  requestCard(request) {
    if (request.type === 'elicitation') return NativeSdkControls.elicitation(this,request);
    const card = el('div', 'msg sdk-request'); card.dataset.sdkRequest = request.key;
    card.dataset.role = 'question';
    const body = el('div', 'mb'); card.append(body);
    body.append(el('p', 'sdk-request-title', request.title || (request.type === 'approval' ? '需要批准' : '需要回答')));
    if (request.description) body.append(el('pre', '', request.description));
    if (request.type === 'user_dialog') body.append(el('p','sdk-control-note',`模型：${request.payload?.originalModel || '当前模型'} → ${request.payload?.fallbackModel || '备用模型'}`));
    if (request.input || request.resources) body.append(el('pre','',JSON.stringify(request.input || request.resources,null,2)));
    const actions = el('div', 'sdk-request-actions');
    const add = (label, value) => {
      const button = el('button', 'btn', label); button.type = 'button';
      button.onclick = () => this.answer(request, typeof value === 'function' ? value() : value, card);
      actions.append(button);
    };
    const fields = request.questions || request.fields;
    if (request.type === 'question' && fields?.length) {
      const readers = fields.map((field,index) => {
        const group = document.createElement('fieldset'); group.className = 'sdk-question';
        const title = field.question || field.title || field.key || '回答';
        group.append(el('legend','',[field.header,title].filter(Boolean).join(' · ')));
        const multiple = field.multiSelect || field.multiple || field.type === 'array';
        for (const option of field.options || []) {
          const label = document.createElement('label'), input = document.createElement('input');
          const value = typeof option === 'string' ? option : option.value ?? option.label ?? option.id;
          input.type = multiple ? 'checkbox' : 'radio'; input.name = `native-${request.key}-${index}`;
          input.value = String(value);
          label.append(input, ` ${typeof option === 'string' ? option : option.label || option.title || String(value)}`);
          if (option.description) label.title = option.description;
          group.append(label);
        }
        const other = document.createElement('input'); other.type = 'text';
        other.placeholder = (field.options || []).length ? '其他答案' : '输入答案';
        other.setAttribute('aria-label',title); group.append(other); body.append(group);
        return () => {
          const picked = [...group.querySelectorAll('input:checked')].map(input => input.value);
          const typed = other.value.trim();
          let value = multiple ? (typed ? [...picked,typed] : picked) : typed || picked[0] || '';
          if (request.questions && Array.isArray(value) && !this.uid.startsWith('grok:')) value = value.join(', ');
          if (field.type === 'number' || field.type === 'integer') value = value === '' ? '' : Number(value);
          if (field.type === 'boolean') value = value === 'true';
          return [field.key || field.question || String(index),value];
        };
      });
      add('提交', () => {
        const answers = Object.fromEntries(readers.map(read => read()));
        return request.questions ? {answers} : {answer:answers};
      });
      if (request.questions) add('拒绝', {behavior:'deny'});
      body.append(actions); return card;
    }
    for (const option of request.options || []) add(option.label || option.name || String(option.value), option.value ?? option.id ?? option.label);
    if (request.type === 'question' || !(request.options || []).length) {
      const input = document.createElement('textarea'); input.className = 'sdk-json'; input.setAttribute('aria-label','回答'); body.append(input);
      add('提交', () => input.value);
    }
    body.append(actions); return card;
  },

  async answer(request, result, card) {
    if (request.type === 'user_dialog' && typeof result === 'string') result={behavior:'completed',result};
    const generation = this.generation;
    card.querySelectorAll('button,input,textarea').forEach(control => { control.disabled = true; });
    try { await this.request('api/native-sdk/respond', {uid:this.uid,key:request.key,result}); }
    catch (error) {
      if (this.current(generation)) {
        card.querySelectorAll('button,input,textarea').forEach(control => { control.disabled = false; });
        this.setNotice(`回答失败：${error.message}`, 'blocked');
      }
    }
  },

  async send() {
    if (!this.input || !this.pane) return;
    const generation = this.generation, button = this.pane.querySelector('#sdk-send');
    if (button.disabled) return;
    button.disabled = true;
    button.setAttribute('aria-busy','true');
    try {
      if (await NativeSdkControls.command(this, this.input.value)) return;
      const payload = await CodexSdkCompose.collect(this);
      if (!payload || !this.current(generation)) return;
      const request_id = await CodexSdkCompose.submission(payload,null,null,this.agent);
      const response = await this.request('api/native-sdk/turn', {uid:payload.uid,text:payload.text,attachments:payload.attachments,quotes:payload.quotes,
        request_id,draft_revision:payload.draft_revision});
      if (!await CodexSdkCompose.accepted(this,payload,{raw:response.raw || response},request_id)) return;
      if ((response.accepted ?? response.raw?.accepted) === false)
        throw new Error(response.error || response.raw?.error || '这次提交未进入发送队列，请查看队列记录');
      if(this.current(generation)){
        this.paintPickers();
      }
      await CodexSdkCompose.sent(this,payload);
    } catch (error) { if (this.current(generation)) this.setNotice(`发送失败：${error.message}`, 'blocked'); }
    finally { if (this.current(generation)) CodexSdkCompose.failed(this); button.disabled = false;button.removeAttribute('aria-busy');if(this.current(generation))sdkSyncSettings(this); }
  },
  async interrupt() {
    const generation = this.generation;
    try { await this.request('api/native-sdk/interrupt', {uid:this.uid}); }
    catch (error) { if (this.current(generation)) this.setNotice(`中断失败：${error.message}`, 'blocked'); }
  },
};

function nativeSdkMessage(item) {
  const message = {...item,text:item.text ?? item.content ?? ''};
  if (typeof message.text !== 'string') message.text = JSON.stringify(message.text,null,2);
  if (item.role === 'tool') {
    message.name = item.name || item.tool_name || 'tool';
    const input = item.input ?? item.tool_input ?? item.arguments;
    if (input != null) message.text = typeof input === 'string' ? input : JSON.stringify(input,null,2);
    const result = item.result ?? item.tool_result;
    if (result != null && !(typeof result === 'object' && result.role === 'tool_result')) {
      message.result = {role:'tool_result',text:typeof result === 'string' ? result : JSON.stringify(result,null,2),
        error:!!item.error || !!item.is_error || item.status === 'failed'};
    } else if (result == null && (item.tool_content || item.status === 'failed')) {
      message.result = {role:'tool_result',text:typeof item.content === 'string' ? item.content : JSON.stringify(item.tool_content),error:!!item.error};
    }
  }
  return message;
}

// Existing history/navigation code talks to the selected view through this
// facade. Codex-specific commands and protocol rendering keep their own view.
function savedSessionView(uid, agent) {
  const key = viewKey(uid, agent);
  const saved = store.get('sessionViews', {})?.[key]
    ?? (String(uid).startsWith('claude:') ? store.get('claudeSessionViews', {})?.[key] : null);
  return ['sdk', 'chat', 'terminal'].includes(saved) ? saved : null;
}
function rememberSessionView(uid, agent, mode) {
  const saved = store.get('sessionViews', {}) || {};
  saved[viewKey(uid, agent)] = mode;
  store.set('sessionViews', saved);
}
function defaultSessionView(uid) {
  return agentStartMode(String(uid).split(':')[0]) === 'sdk' ? 'sdk' : 'chat';
}

const SdkView = new Proxy({
  active: CodexSdkView,
  awaitingRunning: false,
  legacyViews: CodexSdkView.legacyViews,
  available(uid, agent) { return CodexSdkView.available(uid,agent) || NativeSdkView.available(uid,agent); },
  preferredMode(uid, agent) {
    const saved = savedSessionView(uid, agent);
    if (saved) return saved;
    if (!String(uid).startsWith('claude:') || agent) return defaultSessionView(uid);
    // Discovery may still be in flight on reload. Do not start a competing
    // SDK process before we know whether this session already has a console.
    if (typeof T === 'undefined' || !T.listLoaded) return 'chat';
    const name = takenOver(uid);
    if (name) return T.openViews.get(name)?.mode === 'collapsed' ? 'chat' : 'terminal';
    if (S.live.has(uid) && !S.liveSdk.has(uid)) return 'chat';
    return defaultSessionView(uid);
  },
  rememberMode(uid, agent, mode) {
    rememberSessionView(uid, agent, mode);
  },
  defaultFor(uid, agent) {
    const mode = this.preferredMode(uid, agent);
    return this.available(uid,agent) && (mode ? mode === 'sdk' : !this.legacyViews.has(viewKey(uid,agent)));
  },
  pending(uid) { return SdkUI.methods.pending(uid); },
  syncRunning() {
    const view = this.active;
    if (!this.awaitingRunning || !view.pane || !view.readOnly || view.agent || view.uid !== S.sel || S.agent
        || SessionContentView.kind !== 'sdk' || !S.liveSdk.has(view.uid)) return;
    // Discovery can arrive after history was opened read-only on reload.
    // Attach to the observed running instance without replacing history/drafts.
    view.readOnly = false;
    view.pane.querySelector('#sdk-composer').hidden = false;
    void view.start(view.generation);
  },
  retryOpen() {
    this.syncRunning();
    this.active.retryOpen();
  },
  open(uid,pane,agent,options) {
    this.close(); this.legacyViews.delete(viewKey(uid,agent));
    this.awaitingRunning = true;
    this.rememberMode(uid, agent, 'sdk');
    this.active = String(uid).startsWith('codex:') ? CodexSdkView : NativeSdkView;
    const running = !agent && S.liveSdk.has(uid);
    this.active.open(uid,pane,agent,{...options, readOnly: !options?.start && !running});
    closeWatch();
  },
  async startSession(uid, agent = null) {
    if (SessionContentView.kind !== 'sdk' || this.uid !== uid || this.agent !== agent)
      SessionContentView.open('sdk', uid, agent);
    if (this.active.readOnly) {
      this.active.readOnly = false;
      this.pane.querySelector('#sdk-composer').hidden = false;
    }
    await this.active.start(this.active.generation);
    await refreshLive(true);
    refreshSessionRunControls();
  },
  stopped(uid, selectedAgent = null) {
    if (!selectedAgent) { S.liveSdk.delete(uid); S.live.delete(uid); }
    if (this.uid === uid && this.agent === selectedAgent && this.pane) {
      // This is an intentional stop, not initial discovery still in flight.
      this.awaitingRunning = false;
      const pane = this.pane, agent = this.agent;
      this.active.open(uid, pane, agent, {readOnly: true});
    }
    refreshSessionRunControls();
    paintLive();
  },
  close() {
    const uid = this.active.uid, agent = this.active.agent;
    CodexSdkView.close(); NativeSdkView.close();
    if (uid) queueMicrotask(() => {
      if (!this.active.pane && S.sel === uid && S.agent === agent) watchSession(uid, agent);
    });
  },
}, {
  get(target,key,receiver) {
    if (key in target) return Reflect.get(target,key,receiver);
    const value = target.active[key];
    return typeof value === 'function' ? value.bind(target.active) : value;
  },
  set(target,key,value) {
    if (key in target) target[key] = value;
    else target.active[key] = value;
    return true;
  },
});
