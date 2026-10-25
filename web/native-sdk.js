'use strict';

// Native providers share the SDK surface, draft/upload store and history
// renderer. Protocol translation and process ownership stay on the node.
const NATIVE_SDK_SOURCES = Object.freeze({
  claude: {source: 'claude', title: 'Claude', emptyTitle: '新建 Claude 会话', busyCode: 'native_sdk_session_busy'},
  grok: {source: 'grok', title: 'Grok', emptyTitle: '新建 Grok 会话', busyCode: 'native_sdk_session_busy'},
  opencode: {source: 'opencode', title: 'OpenCode', emptyTitle: '新建 OpenCode 会话', busyCode: 'native_sdk_session_busy'},
  agy: {source: 'agy', title: 'Agy', emptyTitle: '新建 Agy 会话', busyCode: 'native_sdk_session_busy'},
  kiro: {source: 'kiro', title: 'Kiro', emptyTitle: '新建 Kiro 会话', busyCode: 'kiro_sdk_session_busy'},
  codebuddy: {source: 'codebuddy', title: 'CodeBuddy', emptyTitle: '新建 CodeBuddy 会话', busyCode: 'native_sdk_session_busy'},
});

const NativeSdkView = {
  ...SdkUI.methods,
  sources: NATIVE_SDK_SOURCES,
  create(source) {
    const spec = NATIVE_SDK_SOURCES[source] || {
      source,
      title: (typeof SOURCES !== 'undefined' && SOURCES[source]?.name) || source,
      emptyTitle: `新建 ${(typeof SOURCES !== 'undefined' && SOURCES[source]?.name) || source} 会话`,
      busyCode: source === 'kiro' ? 'kiro_sdk_session_busy' : 'native_sdk_session_busy',
    };
    const cli = typeof SESSIONDOCK_CLIS !== 'undefined' ? SESSIONDOCK_CLIS[source] : null;
    return cli ? Object.assign(cli, spec) : spec;
  },
  uid: '', agent: null, pane: null, generation: 0, connected: false,
  available(uid, agent) {
    const source = String(uid || '').split(':')[0];
    return !(agent && ['claude', 'kiro', 'codebuddy'].includes(source)) && SessionDockCapabilities.config.native_sdks?.[source] === true
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
    this.liveTurns = new Map();
    this.nativeAgents = null; this.loadingUsage = false; this.raw = {}; this.usageData = {}; this.usageError = null;
    this.models = []; this.capabilities = {}; this.running = false; this.seq = -1;
    this.historyRenderedData = null; this.historyCut = null;
    this.nativeModel = this.nativeEffort = null;
    const generation = ++this.generation;
    const source = String(uid).split(':')[0];
    const name = SOURCES[source]?.name || NATIVE_SDK_SOURCES[source]?.title || 'Agent';
    pane.classList.add('native-sdk-pane');
    SdkUI.mount(this, {name, readOnly, historyReady,
      settings: (view, values) => NativeSdkControls.settings(view, values)});
    NativeSdkControls.mount(this);
    this.mountToolbar(generation);
    CodexSdkCompose.mount(this);
    void this.refreshHistory(generation);
    if (!this.observeExternal()) void this.start(generation);
  },

  observeExternal() {
    if (!this.pane) return false;
    const external = S.liveExternalSdk.has(this.uid);
    if (external) {
      this.externalObserved = true;
      this.readOnly = true;
      this.input.disabled = true;
      this.input.onclick = null;
      this.input.placeholder = '外部会话仅可查看，请在原程序中发送消息';
      this.pane.querySelector('#sdk-send').disabled = true;
      CodexSdkCompose.paint(this);
      this.setNotice('外部只读：ACP 会话由原程序控制，历史自动更新。', 'neutral');
    } else if (this.externalObserved) {
      this.externalObserved = false;
      this.input.placeholder = '通过 Kiro SDK 发送';
      this.input.onclick = () => startComposerSession(this.uid, this.agent, 'sdk');
      CodexSdkCompose.paint(this);
      this.setNotice('未运行', 'idle');
    }
    return external;
  },

  close() {
    this.externalObserved = false;
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
    this.setNotice(reconnecting ? '重连中' : '连接中', 'connecting');
    try {
      const data = await this.request('api/native-sdk/open', {uid:this.uid}, read.signal);
      if (!this.current(generation)) return;
      if (String(this.uid).startsWith('kiro:') && typeof findTermForSession === 'function'
          && findTermForSession(this.uid) && !S.liveSdk.has(this.uid) && !data?.capabilities && !data?.raw?.capabilities) {
        const busyError = new Error('CLI 终端会话正在运行，请先停止 CLI 终端会话后再启动 SDK 模式。');
        busyError.code = 'kiro_sdk_session_busy';
        throw busyError;
      }
      this.capabilities = data.capabilities || data.raw?.capabilities || {};
      this.openFailed = false;
      void this.loadModels(generation);
      if (this.capabilities.agents) void this.request('api/native-sdk/agents',{uid:this.uid}).then(response => {if(this.current(generation)){this.nativeAgents=response.agents || response.raw?.agents || response.raw?.data || response.raw;NativeSdkControls.paintSettings(this);}}).catch(()=>{});
      void NativeSdkControls.loadUsage(this, /^(agy|kiro):/.test(this.uid) ? 'auto' : true);
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
        const kiroBusy = error.code === 'kiro_sdk_session_busy'
          || (String(this.uid).startsWith('kiro:')
            && (error.code === 'native_sdk_session_busy'
              || (typeof findTermForSession === 'function' && !!findTermForSession(this.uid))));
        if (kiroBusy) {
          this.readOnly = true;
          this.syncComposer?.();
          this.setNotice(error.message || 'CLI 终端会话正在运行，请先停止 CLI 终端会话后再启动 SDK 模式。', 'blocked');
        } else {
          this.setNotice(`无法打开 SDK 模式：${error.message}`, 'blocked');
          if (reconnecting || error.code === 'native_sdk_session_busy') this.reconnect();
        }
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
      this.models = nativeSdkModelCatalog(response.raw || response, '').models;
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
    const kiroEffortUnavailable = this.uid.startsWith('kiro:') &&
      (this.raw?.capabilities?.effort !== true || selected?.hasEffort === false);
    const efforts = kiroEffortUnavailable ? [] : selected?.supportedReasoningEfforts || selected?.efforts || [];
    const effort = this.pane.querySelector('#sdk-effort');
    effort.replaceChildren();
    for (const row of efforts) { const id = typeof row === 'string' ? row : row.reasoningEffort || row.id; if (id != null && ![...effort.options].some(option=>option.value===id)) effort.add(new Option(id ? UsageUI.effort(id) : 'CLI 默认', id)); }
    // A native config may remove this control while accepting a settings
    // request. Observe that normalization so its receipt can release the draft.
    if (kiroEffortUnavailable && this.raw?.capabilities?.effort === false) {
      const pending = this.settingChoices.get('effort');
      if (pending) pending.observed = true;
    }
    let chosen=sdkSettingValue(this,'effort',kiroEffortUnavailable ? '' : this.nativeEffort || '');
    if (kiroEffortUnavailable) chosen='';
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
    SdkUI.ungroupTools(this);
    const stick = this.nearBottom();
    const completed = this.running && !raw.running;
    const observedModelChanged = this.raw?.model_observed !== raw.model_observed;
    const effortCapabilityChanged = this.uid.startsWith('kiro:') && this.raw?.capabilities?.effort !== raw.capabilities?.effort;
    const kiroConfigChanged = this.uid.startsWith('kiro:') && JSON.stringify(this.raw?.configOptions) !== JSON.stringify(raw.configOptions);
    this.raw = raw;
    this.running = !!raw.running;
    const model = typeof raw.model === 'string' ? raw.model : raw.model?.providerID && raw.model?.id
      ? `${raw.model.providerID}/${raw.model.id}` : raw.model?.id || raw.model?.modelID || null;
    const effort = raw.effort || null;
    if (model !== this.nativeModel || effort !== this.nativeEffort || observedModelChanged || effortCapabilityChanged) {
      this.nativeModel = model; this.nativeEffort = effort; this.paintPickers();
    }
    this.openFailed = !!raw.error;
    this.capabilities = {...this.capabilities, ...raw.capabilities};
    if (kiroConfigChanged) void this.loadModels(this.generation);
    const rawItems = Array.isArray(raw.items) ? raw.items : [];
    this.items = (this.uid.startsWith('kiro:')
      ? rawItems.filter(item => !(item?.role === 'system' && (item.kind === 'compact' || item.type === 'compact')))
      : rawItems).map(nativeSdkMessage);
    this.setConnected(true);
    this.paintHistory();
    const ids = new Set();
    for (const item of this.items) {
      const id = String(item.id); ids.add(id);
      const old = this.nodes.get(id);
      if (old?._updateMessage && old.dataset.role === item.role) { old._updateMessage(item); continue; }
      const toolState = item.role === 'tool' ? ConversationUI.toolState(old) : null;
      const node = msgNode(item); node.dataset.sdkItem = id;
      node._sdkToolMessage = item;
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
    SdkUI.foldLiveTurns(this, this.items, {tailComplete: !this.running});
    SdkUI.groupTools(this);
    const requests = new Map((raw.requests || []).map(request => [request.key, request]));
    for (const [key] of this.requests) if (!requests.has(key)) this.live.querySelector(`[data-sdk-request="${CSS.escape(key)}"]`)?.remove();
    for (const [key,request] of requests) if (!this.requests.has(key)) this.live.append(this.requestCard(request));
    this.requests = requests;
    this.pane.querySelector('#sdk-stop').hidden = (!this.running && !this.raw?.retry?.delay_seconds) || this.capabilities.interrupt === false;
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
    if(this.uid.startsWith('kiro:'))send.title+='；Ctrl+Enter 引导当前任务';
    SdkUI.paintSend(this);
    const retryReason=raw.retry?.error?.message || raw.retry?.message || raw.retry?.reason;
    const scheduledRetry=raw.retry?.delay_seconds;
    const retryText=scheduledRetry ? `正在重试：${scheduledRetry} 秒后自动重试${raw.retry.retry_at ? `（${new Date(raw.retry.retry_at).toLocaleTimeString()}）` : ''}；可停止重试。${retryReason || ''}` : null;
    const paused=!this.running && !this.requests.size && ['failed','unknown'].includes(pending?.state);
    const compacting = raw.status === 'compacting' || raw.compact?.state === 'compacting';
    const uncertain = raw.error_kind === 'uncertain';
    const observed = observedConversationActivity(this.historyData?.activity, this.uid, this.agent);
    // Claude's history activity closes on turn_duration, which can arrive
    // after its assistant end_turn. The native final phase is already proof
    // of completion; a later user/tool item prevents borrowing that proof.
    const tail=this.historyData?.messages?.findLast(item=>['user','assistant','thinking','tool','tool_result'].includes(item.role));
    const historyCompleted=this.uid.startsWith('claude:') && tail?.role==='assistant'
      && tail.phase==='final' && !tail.interrupted;
    const previous = raw.last_turn?.status || (raw.error_kind === 'interrupted' ? 'interrupted'
      : observed.activity?.state === 'aborted' ? 'interrupted'
      : observed.activity?.state === 'failed' ? 'failed'
      : historyCompleted ? 'completed'
      : this.historyData?.activity?.state === 'working' && !S.liveWorking.has(this.uid)
        && cache.get(viewKey(this.uid, this.agent))?.cli?.instance?.busy !== true ? 'incomplete' : '');
    const ended = !this.running && !this.requests.size && !pending;
    const incomplete = ended && ['interrupted','incomplete','unknown'].includes(previous);
    const failed = ended && previous === 'failed';
    const compactFailed = raw.compact?.state === 'failed';
    const error = raw.error || (compactFailed ? raw.compact.error || '上下文压缩失败' : null)
      || (failed ? raw.last_turn?.error || observed.activity?.error || '上一回合失败' : null);
    const warning = uncertain || raw.error_kind === 'interrupted' || incomplete;
    const kind = scheduledRetry ? 'retrying' : error ? (warning ? 'warning' : 'blocked') : this.requests.size ? 'waiting' : compacting ? 'maintenance'
      : raw.retry ? 'retrying' : this.running ? 'working' : paused || incomplete ? 'warning' : pending ? 'waiting' : 'ready';
    const outcome = previous === 'interrupted' ? '已中断' : previous === 'unknown' ? '结果待核对' : '未完成：上一回合未正常结束';
    let text = scheduledRetry ? retryText : error ? `${warning ? (uncertain ? '结果待核对' : outcome) : '错误'}：${typeof error === 'string' ? error : error.message || JSON.stringify(error)}`
      : this.requests.size ? ([...this.requests.values()].some(request=>request.type==='approval')?'待批准':'待回答')
      : compacting ? '压缩中' : raw.retry ? `正在重试${raw.retry.attempt ? `（${raw.retry.attempt}）` : ''}${retryReason?`：${retryReason}`:''}`
      : this.running ? '处理中' : paused ? '队列暂停，请处理失败或待核对记录' : pending ? '等待发送'
      : incomplete ? outcome : raw.compact?.state === 'success' ? '压缩完成' : '就绪';
    if (raw.runtime_error) text += `\n运行实例：${raw.runtime_error}`;
    this.setNotice(text, kind);
  },

  paintHistory() {
    if (!this.history || !this.historyData) return;
    let messages = this.historyData.messages || [];
    let cut = messages.length;
    const first = this.items?.find(item => item.role === 'user');
    if (first) {
      const identity = NativeSdkControls.messageIdentity(this, first);
      const at = messages.findIndex(message => message.role === 'user' && identity
        && NativeSdkControls.messageIdentity(this, message) === identity);
      if (at >= 0) { cut = at; messages = messages.slice(0, at); }
    }
    const actions=JSON.stringify([this.capabilities.fork,this.capabilities.fork_at_message,this.capabilities.revert,this.capabilities.rewind]);
    if (this.historyRenderedData === this.historyData && this.historyCut === cut && this.historyActions===actions) return;
    this.historyActions=actions;
    this.historyRenderedData = this.historyData; this.historyCut = cut;
    SdkUI.paintHistory(this, messages, {decorate:(node, message) => NativeSdkControls.decorate(this,node,message)});
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
      const readers = SdkUI.appendQuestionFields(body, fields, {
        namePrefix: `native-${request.key}`,
        formatAnswer: (field, index, {picked, typed, multiple}) => {
          let value = multiple ? (typed ? [...picked, typed] : picked) : typed || picked[0] || '';
          if (request.questions && Array.isArray(value) && !this.uid.startsWith('grok:')) value = value.join(', ');
          if (field.type === 'number' || field.type === 'integer') value = value === '' ? '' : Number(value);
          if (field.type === 'boolean') value = value === 'true';
          return [field.key || field.question || String(index), value];
        },
      });
      add('提交', () => {
        const answers = Object.fromEntries(readers.map(read => read()));
        return request.questions ? {answers} : {answer:answers};
      });
      if (request.questions) add('拒绝', {behavior:'deny'});
      body.append(actions); return card;
    }
    if (request.type === 'approval' && this.uid.startsWith('kiro:') && !(request.options || []).length) {
      body.append(el('p', 'sdk-control-note', 'Kiro 未提供可选的审批项，无法批准此请求。'));
      add('取消', 'cancelled');
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
    try {
      await this.request('api/native-sdk/respond', {uid:this.uid,key:request.key,result});
      card.classList.add('sdk-answered');
    }
    catch (error) {
      if (this.current(generation)) {
        card.querySelectorAll('button,input,textarea').forEach(control => { control.disabled = false; });
        this.setNotice(`回答失败：${error.message}`, 'blocked');
      }
    }
  },

  async send(operation = 'turn') {
    if (!this.input || !this.pane) return;
    if (operation === 'steer' && this.uid.startsWith('kiro:') && !this.running) operation = 'turn';
    const steer = operation === 'steer';
    if (steer && (!this.uid.startsWith('kiro:') || !this.running || !this.capabilities.steer)) {
      this.setNotice('当前没有可引导的运行中任务；请用 Enter 开始新回合。', 'blocked'); return;
    }
    const generation = this.generation, button = this.pane.querySelector('#sdk-send');
    if (button.disabled) return;
    button.disabled = true;
    button.setAttribute('aria-busy','true');
    let steerRequest;
    const auditUid=this.uid;
    try {
      if (!steer && await NativeSdkControls.command(this, this.input.value)) return;
      const payload = await CodexSdkCompose.collect(this);
      if (!payload || !this.current(generation)) return;
      if (steer && payload.attachments.length) throw new Error('Kiro 原生引导只支持文字；请移除附件或使用普通排队发送。');
      const request_id = await CodexSdkCompose.submission(payload,null,null,this.agent,steer ? 'steer' : undefined);
      if(steer)steerRequest=request_id;
      if(steer)browserAuditEvent('sdk.steer.requested',{source:'kiro'},{uid:payload.uid,requestId:request_id});
      const response = await this.request(`api/native-sdk/${operation}`,  {uid:payload.uid,text:payload.text,attachments:payload.attachments,quotes:payload.quotes,
        request_id,draft_revision:payload.draft_revision});
      if (!await CodexSdkCompose.accepted(this,payload,{raw:response.raw || response},request_id)) return;
      if ((response.accepted ?? response.raw?.accepted) === false)
        throw new Error(response.error || response.raw?.error || '这次提交未进入发送队列，请查看队列记录');
      if(this.current(generation)){
        this.paintPickers();
      }
      await CodexSdkCompose.sent(this,payload);
      if(steer)browserAuditEvent('sdk.steer.finished',{source:'kiro',outcome:'native_queued'},{uid:auditUid,requestId:steerRequest});
      if(steer && this.current(generation))this.setNotice('原生已接收引导；尚未确认处理。');
    } catch (error) {
      if(steer)browserAuditEvent('sdk.steer.finished',{source:'kiro',outcome:'failed'},{uid:auditUid,requestId:steerRequest});
      if (this.current(generation)) this.setNotice(`发送失败：${error.message}`, 'blocked');
    }
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
  const saved = store.get('sessionViews', {})?.[key];
  const hasChat = typeof sessionHasChatView === 'function' ? sessionHasChatView(uid) : sessionSourceOf(uid) !== 'kiro';
  if (!hasChat) return ['sdk', 'terminal'].includes(saved) ? saved : null;
  return ['sdk', 'chat', 'terminal'].includes(saved) ? saved : null;
}
function rememberSessionView(uid, agent, mode) {
  const hasChat = typeof sessionHasChatView === 'function' ? sessionHasChatView(uid) : sessionSourceOf(uid) !== 'kiro';
  const normalized = !hasChat && mode === 'chat' ? defaultSessionView(uid) : mode;
  const saved = store.get('sessionViews', {}) || {};
  saved[viewKey(uid, agent)] = normalized;
  store.set('sessionViews', saved);
}
function defaultSessionView(uid) {
  const source = typeof sessionSourceOf === 'function' ? sessionSourceOf(uid) : String(uid || '').split(':')[0];
  const hasChat = typeof sessionHasChatView === 'function' ? sessionHasChatView(source) : source !== 'kiro';
  if (!hasChat) {
    if (typeof sessionHasRunningSdk === 'function' && sessionHasRunningSdk(uid)) return 'sdk';
    if (typeof findTermForSession === 'function' && findTermForSession(uid)) return 'terminal';
    const key = typeof viewKey === 'function' ? viewKey(uid, null) : String(uid || '');
    const explicit = (typeof SdkView !== 'undefined' && SdkView.views?.get(key)) || savedSessionView(uid, null);
    if (explicit === 'sdk' || explicit === 'terminal') return explicit;
    const sdkAvail = String(uid || '').includes(':') && typeof SdkView !== 'undefined'
      ? SdkView.available(uid, null)
      : (typeof nativeSdkAvailableFor === 'function' ? nativeSdkAvailableFor(source) : false);
    if (agentStartMode(source) === 'cli' || !sdkAvail) return 'terminal';
    return 'sdk';
  }
  return agentStartMode(source) === 'sdk' ? 'sdk' : 'chat';
}

const SdkView = new Proxy({
  active: CodexSdkView,
  awaitingRunning: false,
  discovering: false,
  selectController: null,
  available(uid, agent) { return CodexSdkView.available(uid,agent) || NativeSdkView.available(uid,agent); },
  views: new Map(),
  selection: null,
  preferredMode(uid, agent) {
    return this.selection?.key === viewKey(uid, agent) ? this.selection.mode : null;
  },
  selectMode(uid, agent, mode) {
    const hasChat = typeof sessionHasChatView === 'function' ? sessionHasChatView(uid) : sessionSourceOf(uid) !== 'kiro';
    const nextMode = !hasChat && mode === 'chat' ? defaultSessionView(uid) : mode;
    const key = viewKey(uid, agent);
    this.views.set(key, nextMode);
    rememberSessionView(uid, agent, nextMode);
    if (!agent) this.selection = {...(this.selection?.key === key ? this.selection : {}), key, mode: nextMode};
  },
  async selectFor(uid, agent) {
    this.selectController?.abort();
    this.selectController = null;
    const hasChat = typeof sessionHasChatView === 'function' ? sessionHasChatView(uid) : sessionSourceOf(uid) !== 'kiro';
    const key = viewKey(uid, agent);
    if (agent || (!this.available(uid, agent) && (!HUB_MODE || Nodes.capabilities[nodeOf(uid)]))) {
      if (!hasChat && !agent) {
        const mode = 'terminal';
        this.selection = {key, mode, runningSdk: false};
        return mode;
      }
      this.selection = null;
      return;
    }
    const rawCandidate = this.views.get(key) || savedSessionView(uid, agent);
    const candidate = !hasChat && rawCandidate === 'chat' ? null : rawCandidate;
    const selection = this.selection = {key, mode: 'loading'};
    const controller = this.selectController = new AbortController();
    const timer = setTimeout(() => controller.abort(), 12000);
    try {
      // Choosing the view mode needs the run-status of THIS one session, not a
      // whole-fleet scan. Ask the per-session endpoint (O(1): in-memory SDK
      // status + the last cached live membership) instead of forcing a full
      // /api/live + /api/term/list, which cost ~1.5s on a large fleet for no
      // decision benefit. Terminal presence comes from the cached term list.
      // A background full refresh self-heals any staleness off the hot path.
      // A reload can restore a selection before the deferred term.js defines
      // T. Wait for it (poll too: DOMContentLoaded may already have fired)
      // before reading terminal state; warm switches never wait here.
      if (typeof T === 'undefined' && document.readyState !== 'complete')
        await new Promise(resolve => {
          const done = () => { clearInterval(poll); resolve(); };
          const poll = setInterval(() => { if (typeof T !== 'undefined' || document.readyState === 'complete') done(); }, 20);
          document.addEventListener('DOMContentLoaded', done, {once: true});
        });
      const query = new URLSearchParams({uid});
      if (HUB_MODE) query.set('nodes', nodeOf(uid));
      let state = null;
      try {
        const response = await fetch(appUrl('api/session/runtime?' + query), {signal: controller.signal});
        if (response.ok) state = await response.json();
      } catch { /* unknown run-status falls back to the saved/default view */ }
      // A reload or deep link may beat the first terminal list (and, on a Hub,
      // capability discovery). Wait for that one load before deciding; warm
      // switches skip it and read only the in-memory list.
      // A superseded list request resolves without loading; follow the newer
      // in-flight one (bounded) instead of deciding on an unloaded list.
      for (let tries = 0; tries < 4 && typeof T !== 'undefined'
          && (!T.listLoaded || (HUB_MODE && !Nodes.capabilities[nodeOf(uid)])); tries++) {
        await loadTermList(false).catch(() => {});
        if (controller.signal.aborted) break;
      }
      // Self-heal staleness off the critical path: a background full-fleet
      // refresh converges a just-changed terminal/external owner shortly, and
      // updates the per-session cache the next GET reads, without delaying the
      // view. Never awaited.
      const bgQuery = new URLSearchParams({force: '1'});
      if (HUB_MODE) bgQuery.set('nodes', nodeOf(uid));
      void fetch(appUrl('api/live?' + bgQuery)).then(r => r.ok && r.json()).then(() => {
        if (typeof paintLive === 'function') paintLive();
      }).catch(() => {});
      // The background refresh may reveal a managed console that the cached
      // reads did not know yet; reopen once so ownership picks it up.
      if (typeof T !== 'undefined') void loadTermList(true).then(() => {
        // Opening the chosen view rebuilds the selection object; compare its
        // key and mode so a later explicit view change is left alone.
        if (S.sel !== uid || S.agent !== agent || this.selection?.key !== key
            || this.selection.mode !== selection.mode || !selection.withConsole) return;
        if (selection.withConsole !== selection.mode && typeof findTermForSession === 'function'
            && findTermForSession(uid) && typeof openSession === 'function')
          void openSession(uid, agent, {exact: true, historyMode: 'none'});
      }).catch(() => {});
      if (this.selection !== selection || S.sel !== uid || S.agent !== agent) return;
      if (!this.available(uid, agent)) {
        if (!hasChat && !agent) {
          this.selection = {key, mode: 'terminal', runningSdk: false};
          return 'terminal';
        }
        this.selection = null;
        return;
      }
      // Everything the mode decision needs is in the per-session GET: `sdk`
      // (fresh, authoritative), `tmux` (a running terminal instance owns it),
      // `external` (a non-SDK process owns it). No list fetch, no T.list read.
      // A failed GET means run-status is unknown -> saved/default view.
      const unknown = !state;
      const sdk = !unknown && state.sdk === true;
      // The cached live body can predate a just-started managed terminal; the
      // in-memory terminal list (no fetch) covers that until it refreshes.
      const termName = typeof T === 'undefined' ? null : takenOver(uid);
      const termRow = (termName && (T.list || []).find(row => row.name === termName && row.running !== false && !row.stale))
        || (typeof findTermForSession === 'function' ? findTermForSession(uid) : null);
      const console = !!termRow || (!unknown && state.known === true && state.tmux === true);
      const external = !unknown && state.known === true && state.external === true;
      selection.runningSdk = !unknown && sdk === true;
      if (!hasChat) {
        selection.mode = sdk ? 'sdk'
          : console ? 'terminal'
          : candidate || defaultSessionView(uid);
      } else {
        // A previous view is only a candidate. Current server ownership decides
        // whether it can still accept input, including across clients/reloads.
        selection.mode = console ? (candidate === 'sdk' && sdk ? 'sdk' : candidate === 'chat' ? 'chat' : 'terminal')
          : sdk ? 'sdk' : external || unknown || (typeof terminalListUncertain !== 'function' || terminalListUncertain(uid)) ? 'chat' : candidate || defaultSessionView(uid);
      }
      // The mode a console discovered later would select; self-heal above
      // reopens only when it differs from the decision just made.
      if (!console) selection.withConsole = hasChat
        ? (candidate === 'sdk' && sdk ? 'sdk' : candidate === 'chat' ? 'chat' : 'terminal')
        : (sdk ? 'sdk' : 'terminal');
      return selection.mode;
    } finally {
      clearTimeout(timer);
      if (this.selectController === controller) this.selectController = null;
    }
  },
  defaultFor(uid, agent) {
    if (!this.available(uid,agent)) return false;
    return agent ? (this.views.get(viewKey(uid, agent)) || savedSessionView(uid, agent) || 'sdk') === 'sdk'
      : this.preferredMode(uid,agent) === 'sdk';
  },
  pending(uid) { return SdkUI.methods.pending(uid); },
  syncRunning() {
    const view = this.active;
    if (view.observeExternal?.()) return;
    if (this.discovering || !this.awaitingRunning || !view.pane || !view.readOnly || view.agent || view.uid !== S.sel || S.agent
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
    this.close();
    this.awaitingRunning = true;
    this.discovering = options?.discovering === true;
    if (!this.discovering) this.selectMode(uid, agent, 'sdk');
    this.active = String(uid).startsWith('codex:') ? CodexSdkView : NativeSdkView;
    const running = !agent && ((String(uid).startsWith('kiro:') && S.liveSdk.has(uid))
      || (this.selection?.key === viewKey(uid,agent)
        ? this.selection.runningSdk ?? S.liveSdk.has(uid) : S.liveSdk.has(uid)));
    this.active.open(uid,pane,agent,{...options, readOnly: this.discovering || (!options?.start && !running)});
    if (this.discovering) {
      this.active.input.onclick = null;
      this.active.setNotice('确认状态中', 'neutral');
    }
    closeWatch();
  },
  async startSession(uid, agent = null) {
    if (S.liveExternalSdk.has(uid)) return;
    // An explicit start during ownership discovery replaces the read-only
    // preview with the real view; otherwise discovery completion closes it.
    if (SessionContentView.kind !== 'sdk' || this.uid !== uid || this.agent !== agent || this.discovering)
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
    if (this.selection?.key === viewKey(uid, selectedAgent)) this.selection.runningSdk = false;
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
    this.discovering = false;
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

const _sessionViewMode = new Proxy({}, {
  get(_target, prop) {
    if (typeof prop !== 'string') return undefined;
    const key = typeof viewKey === 'function' ? viewKey(prop, null) : prop;
    return SdkView.views.get(key) || savedSessionView(prop, null) || undefined;
  },
  set(_target, prop, value) {
    if (typeof prop === 'string') {
      if (value == null) SdkView.views.delete(typeof viewKey === 'function' ? viewKey(prop, null) : prop);
      else SdkView.selectMode(prop, null, value);
    }
    return true;
  },
  deleteProperty(_target, prop) {
    if (typeof prop === 'string') SdkView.views.delete(typeof viewKey === 'function' ? viewKey(prop, null) : prop);
    return true;
  },
});
Object.assign(globalThis, {
  NATIVE_SDK_SOURCES, NativeSdkView, SdkView, defaultSessionView, _sessionViewMode,
  KiroCli: NativeSdkView.create('kiro'),
  CodebuddyCli: NativeSdkView.create('codebuddy'),
});
