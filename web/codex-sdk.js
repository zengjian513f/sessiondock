'use strict';

// Codex SDK 默认视图（docs/codex-sdk.md）；旧“对话/终端”入口保留为退役过渡。
// 已落盘的历史仍由普通读模型给出，气泡与消息模式完全相同；进行中的回合、
// 审批和提问来自节点与共享 Codex app-server 的连接（/api/codex-sdk/*）。
// 这里只负责呈现与交互，不解析终端画面，也不另存历史。
const CodexSdkView = {
  uid: '', agent: null, pane: null, es: null, generation: 0,
  box: null, history: null, live: null, notice: null, input: null,
  turn: null, items: new Map(), nodes: new Map(), requests: new Map(),
  summary: null, models: [], historyTimer: 0, historyData: null,
  limits: null, resetCards: null, redeemKeys: new Map(),
  limitsRead: null, limitsRetryTimer: 0, limitsRetryDelay: 1000, limitsRecordedAt: null, limitsFresh: false,
  openRead: null, openFailed: false, openRetryTimer: 0, openRetryDelay: 1000,
  toolbarObserver: null, toolbarBindings: null, toolbarWidth: null, toolbarLayout: null,
  modelChoice: null, effortChoice: null,
  pendingAtOpen: false,
  legacyViews: new Set(),

  defaultFor(uid, agent) { return this.available(uid, agent) && !this.legacyViews.has(viewKey(uid, agent)); },
  pending(uid) {
    return typeof T === 'undefined' || S.sessions.some(row => row.uid === uid) ? null
      : T.sdkSessions?.find(row => row.uid === uid) || null;
  },

  available(uid, agent) {
    return SessionDockCapabilities.config.codex_sdk === true
      && (!HUB_MODE || Nodes.capabilities[nodeOf(uid)]?.codex_sdk === true)
      && String(uid || '').startsWith('codex:') && !String(uid).startsWith('tmux:');
  },

  unavailableReason(uid, agent) {
    if (SessionDockCapabilities.config.codex_sdk !== true) return '此服务未启用 Codex SDK 模式。';
    if (!String(uid || '').startsWith('codex:')) return 'SDK 模式只支持 Codex 会话。';
    return '';
  },

  open(uid, pane, agent = null, {historyReady = null} = {}) {
    this.legacyViews.delete(viewKey(uid, agent));
    this.close();
    this.stopped = false;
    this.uid = uid; this.agent = agent; this.pane = pane;
    this.historyReady = historyReady;
    this.pendingAtOpen = !agent && !!this.pending(uid);
    const generation = ++this.generation;
    const remembered=UsageUI.remembered(uid,agent);
    this.limits=remembered?.limits || null;this.cachedUsage=remembered?.usage || null;
    this.limitsRecordedAt=this.limits ? '' : null;
    pane.setAttribute('aria-label', 'Codex SDK');
    pane.classList.add('sdk-pane');
    pane.innerHTML = `<div class="sdk-view">
      <div class="msgs sdk-msgs" aria-live="polite"><div class="sdk-history"></div><div class="sdk-live"></div><div class="sdk-queue"></div></div>
      <div id="sdk-composer">
        <div class="sdk-status-row">
          <button class="sdk-status" type="button" disabled><span role="status" aria-live="polite"></span></button>
          <div class="new-model-menu sdk-toolbar-menu sdk-status-menu" id="sdk-status-menu" popover="auto" role="dialog" aria-label="完整状态"></div>
          <div class="sdk-gauges" aria-label="用量">
            <span class="sdk-gauge" data-gauge="context" hidden></span>
            <span class="sdk-gauge" data-gauge="5h" hidden></span>
            <span class="sdk-gauge" data-gauge="7d" hidden></span>
            <span class="sdk-credits" hidden></span>
          </div>
          <button class="btn" id="sdk-usage-toggle" type="button" aria-expanded="false" aria-controls="sdk-usage-menu" hidden>用量</button>
          <div class="sdk-options">
            <div class="sdk-pickers">
              <label class="new-effort sdk-model sdk-model-fit" title="模型"><span class="visually-hidden">模型</span><select id="sdk-model"><option value="" disabled hidden>模型</option></select></label>
              <label class="new-effort sdk-effort" title="推理强度"><span class="visually-hidden">推理强度</span><select id="sdk-effort"><option value="" disabled hidden>推理强度</option></select></label>
              <button class="btn" id="sdk-more" type="button" title="会话操作" aria-label="会话操作" aria-expanded="false" aria-controls="sdk-more-menu">⋯</button>
              <div class="new-model-menu sdk-toolbar-menu" id="sdk-more-menu" popover="auto" aria-label="会话操作">
                <span class="sdk-reset" hidden>
                  <label class="new-effort" title="可兑换的限额重置卡"><span class="visually-hidden">重置卡</span><select id="sdk-reset-card"></select></label>
                  <button class="btn" id="sdk-reset-use" type="button" title="兑换所选重置卡">兑换</button>
                </span>
              </div>
              <div class="new-model-menu sdk-toolbar-menu sdk-usage-menu" id="sdk-usage-menu" popover="auto" aria-label="用量"></div>
            </div>
          </div>

        </div>
        <div class="composer-row">
          <div class="composer-input-wrap"><textarea id="sdk-input" rows="1" enterkeyhint="enter" placeholder="通过 Codex SDK 发送"></textarea></div>
          <div class="cbtns">
            <button class="btn" id="sdk-stop" type="button" hidden title="中断当前回合">停止</button>
            <button class="btn go" id="sdk-send" type="button">发送</button>
          </div>
        </div>
      </div></div>`;
    this.box = pane.querySelector('.sdk-msgs');
    observeMessageLayout(this.box);
    this.history = pane.querySelector('.sdk-history');
    if (historyReady) this.history.innerHTML = '<div class="spin">正在读取会话…</div>';
    this.live = pane.querySelector('.sdk-live');
    this.notice = pane.querySelector('.sdk-status');
    this.input = pane.querySelector('#sdk-input');
    this.input.oninput = () => { this.fitInput(); CodexSdkCompose.input(this); };
    this.input.onblur = () => PathCompletion.close(this.input);
    this.input.onkeydown = event => {
      if (PathCompletion.key(this.input, event, () => this.uid)) return;
      if (event.key === 'Enter' && !event.shiftKey && !event.isComposing && !MOBILE.matches) {
        event.preventDefault();
        this.send();
      }
    };
    pane.querySelector('#sdk-send').onclick = () => this.send();
    pane.querySelector('#sdk-stop').onclick = () => this.interrupt();
    pane.querySelector('#sdk-model').onchange = event => {
      this.modelChoice = event.target.value || null;
      this.effortChoice = null;
      this.paintPickers();
    };
    pane.querySelector('#sdk-effort').onchange = event => { this.effortChoice = event.target.value || null; };
    pane.querySelector('#sdk-reset-use').onclick = () => this.redeem();
    CodexSdkCompose.mount(this);
    CodexSdkActions.mount(this);
    CodexSdkControls.mount(this);
    this.mountToolbar(generation);
    this.paintPickers();
    this.paintGauges();
    // Disk quota reads do not depend on app-server startup or history loading.
    const quotaRead=new AbortController();this.quotaRead=quotaRead;
    void UsageUI.readQuota(uid,quotaRead.signal).then(quota=>{
      if(!this.current(generation) || this.limitsFresh || this.limits || !quota)return;
      const windows=(quota.buckets || []).filter(row=>row.id?.startsWith('codex:') && Number.isFinite(row.remaining_percent));
      const credit=(quota.buckets || []).find(row=>['codex:credits','credits'].includes(row.id) && row.remaining_text!=null);
      if(!windows.length && !credit)return;
      this.limits={};
      if(credit){
        const unlimited=['不限量','无限','unlimited'].includes(String(credit.remaining_text).toLowerCase());
        const balance=Number(credit.remaining_text);
        if(unlimited || Number.isFinite(balance))this.limits.credits={unlimited,balance:unlimited ? null : balance,hasCredits:balance>0};
      }
      windows.slice(0,2).forEach((row,index)=>this.limits[index?'secondary':'primary']={usedPercent:100-row.remaining_percent,windowDurationMins:UsageUI.minutes(row.window),resetsAt:row.resets_at ? Date.parse(row.resets_at)/1000 : null});
      this.limitsRecordedAt=quota.captured_at || '';this.paintGauges();
    }).catch(()=>{});
    this.refreshHistory(generation);
    this.setNotice('连接中', 'connecting');
    this.start(generation);
  },

  close() {
    if(this.uid)UsageUI.remember(this.uid,this.agent,{limits:this.limits,usage:this.summary && Object.hasOwn(this.summary,"usage") ? this.summary.usage : this.cachedUsage});
    this.quotaRead?.abort();this.quotaRead=null;
    this.setConnected(false);
    if (this.input) PathCompletion.close(this.input);
    CodexSdkCompose.close(this);
    CodexSdkActions.close(this);
    CodexSdkControls.close(this);
    this.generation++;
    this.es?.close();
    this.es = null;
    this.toolbarObserver?.disconnect(); this.toolbarObserver = null;
    disposeMessageObservers(this.box);
    this.toolbarBindings?.abort(); this.toolbarBindings = null;
    this.toolbarWidth = null; this.toolbarLayout = null;
    this.closeToolbarMenus();
    clearTimeout(this.historyTimer);
    clearTimeout(this.openRetryTimer);
    clearTimeout(this.reconnectTimer);
    this.openRead?.abort();
    this.openRead = null; this.openFailed = false; this.openRetryTimer = 0; this.openRetryDelay = 1000;
    clearTimeout(this.limitsRetryTimer);
    this.limitsRead?.abort();
    this.limitsRead = null; this.limitsRetryTimer = 0; this.limitsRetryDelay = 1000;
    this.items.clear(); this.nodes.clear(); this.requests.clear();
    this.turn = null; this.summary = null; this.cachedUsage = null; this.limits = null; this.resetCards = null; this.limitsRecordedAt = null; this.limitsFresh = false;
    this.models = []; this.modelChoice = this.effortChoice = null;
    this.pane = this.box = this.history = this.live = this.notice = this.input = null;
    this.uid = ''; this.agent = null; this.historyData = null; this.historyEntry = null;
    this.historyReady = null; this.historyFailed = false;
  },

  current(generation) { return generation === this.generation && !!this.pane; },

  mountToolbar(generation) {
    this.toolbarBindings = new AbortController();
    for (const name of ['more', 'usage', 'status']) {
      const button = this.pane.querySelector({more: '#sdk-more', usage: '#sdk-usage-toggle', status: '.sdk-status'}[name]);
      const menu = this.pane.querySelector(`#sdk-${name}-menu`);
      menu.addEventListener('beforetoggle', event => button.setAttribute('aria-expanded', String(event.newState === 'open')));
      button.onclick = () => {
        if (button.disabled) return;
        if (menu.matches(':popover-open')) { menu.hidePopover(); return; }
        this.closeToolbarMenus();
        menu.showPopover();
        this.placeMenu(menu, button);
      };
    }
    window.addEventListener('resize', () => this.closeToolbarMenus(), {signal: this.toolbarBindings.signal});
    this.toolbarObserver = new ResizeObserver(() => {
      if (this.current(generation)) this.layoutToolbar(this.pane.querySelector('.sdk-status-row').clientWidth);
    });
    this.toolbarObserver.observe(this.pane.querySelector('.sdk-status-row'));
    this.toolbarObserver.observe(this.notice);
    this.layoutToolbar();
  },

  placeMenu(menu, button, preferredWidth = 300) {
    const box = button.getBoundingClientRect(), zoom = menu.currentCSSZoom || 1;
    const edge = 8, gap = 6, limit = Math.min(preferredWidth, innerWidth - edge * 2);
    const status = menu.classList.contains('sdk-status-menu');
    menu.style.width = status ? 'max-content' : `${limit / zoom}px`;
    if (status) menu.style.maxWidth = `${limit / zoom}px`;
    const width = menu.getBoundingClientRect().width;
    const above = box.top - gap - edge, below = innerHeight - box.bottom - gap - edge;
    const up = above > below;
    menu.style.maxHeight = `${Math.max(0, up ? above : below) / zoom}px`;
    menu.style.left = `${Math.max(edge, Math.min(box.right - width, innerWidth - edge - width)) / zoom}px`;
    menu.style.top = `${(up ? box.top - gap - menu.getBoundingClientRect().height : box.bottom + gap) / zoom}px`;
  },

  closeToolbarMenus() {
    for (const menu of this.pane?.querySelectorAll('.sdk-toolbar-menu, .sdk-bubble-menu') || [])
      if (menu.matches(':popover-open')) menu.hidePopover();
  },

  layoutToolbar(width = this.toolbarWidth) {
    if (!this.pane) return;
    const row = this.pane.querySelector('.sdk-status-row'), gauges = this.pane.querySelector('.sdk-gauges');
    this.toolbarWidth = width ?? row.clientWidth;
    const pickers = this.pane.querySelector('.sdk-pickers'), more = this.pane.querySelector('#sdk-more-menu');
    const usage = this.pane.querySelector('#sdk-usage-toggle');
    row.insertBefore(gauges, usage);
    usage.hidden = true;
    const buttons = ['compact', 'fork', 'revert', 'review'].map(action => this.pane.querySelector(`[data-sdk-action="${action}"]`)).filter(Boolean);
    more.append(this.pane.querySelector('#sdk-permissions'), this.pane.querySelector('.sdk-reset'));
    sdkLayoutFooter(this, ['.sdk-model', '.sdk-effort', '.sdk-tier', '.sdk-mode']
      .map(selector => this.pane.querySelector(selector))
      .concat([...this.pane.querySelectorAll('.sdk-policy')], buttons));
    this.updateStatusDisclosure();
  },

  async request(path, body, signal) {
    const response = await fetch(appUrl(path), {
      method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify({agent: this.agent || '', ...body}),
      signal,
    });
    let data = null;
    try { data = await response.json(); } catch { /* 下面按 HTTP 状态说明 */ }
    if (!response.ok || !data?.ok) throw Object.assign(new Error(data?.error || `请求失败（HTTP ${response.status}）`), {code: data?.code, status: response.status});
    return data;
  },

  async start(generation, reconnecting = false) {
    if (!this.current(generation) || this.openRead || this.es) return;
    clearTimeout(this.openRetryTimer);
    this.openRetryTimer = 0;
    const read = new AbortController();
    this.openRead = read;
    const uid = this.uid;
    // Account reads do not depend on resuming this thread or recovering its turn.
    this.loadLimits(generation);
    try {
      const opened = await this.request('api/codex-sdk/open', {uid}, read.signal);
      if (!this.current(generation)) return;
      // Stopping the conflicting CLI may mark this failed SDK pane stopped.
      // A successful explicit/retry open is a new live attachment, not a SEND.
      if(this.openFailed && this.stopped){this.stopped=false;this.pane.querySelector('#sdk-send').disabled=false;}
      this.summary = opened.raw || {};
      this.seedGauges();
    } catch (error) {
      if (!this.current(generation)) return;
      this.openFailed = true;
      this.setNotice(`无法打开 SDK 模式：${error.message}`, 'blocked', true);
      // An independent writer can exit outside this page. Resume again after
      // it releases the lock; never replay a turn or rebuild the draft/history.
      if (error.code === 'codex_sdk_thread_busy') {
        this.openRetryTimer = setTimeout(() => this.start(generation), this.openRetryDelay);
        this.openRetryDelay = Math.min(this.openRetryDelay * 2, 30000);
      } else if (reconnecting && (!error.status || error.status >= 500)) {
        this.scheduleReconnect();
      }
      return;
    } finally {
      if (this.openRead === read) this.openRead = null;
    }
    if (!this.current(generation)) return;
    this.openFailed = false; this.openRetryDelay = 1000;
    this.setNotice('连接中', 'connecting');
    this.loadModels(generation);
    const es = new EventSource(appUrl(`api/codex-sdk/events?${new URLSearchParams({uid, agent: this.agent || ''})}`));
    this.es = es;
    es.addEventListener('sdk', event => {
      if (!this.current(generation) || this.es !== es) return;
      let packet;
      try { packet = JSON.parse(event.data); } catch { return; }
      if (packet.type === 'snapshot') { this.applySnapshot(packet.raw); return; }
      this.applyEvent(packet.method, packet.raw || {});
    });
    es.onerror = () => {
      if (!this.current(generation) || this.es !== es) return;
      this.setConnected(false);
      if (es.readyState === EventSource.CLOSED) {
        this.setNotice('重连中', 'connecting');
        this.scheduleReconnect();
      }
    };
  },

  reconnect() {
    if (!this.pane) return;
    this.es?.close();
    this.es = null;
    this.start(this.generation, true);
  },

  scheduleReconnect() {
    clearTimeout(this.reconnectTimer);
    const generation = this.generation;
    this.reconnectTimer = setTimeout(() => {
      if (this.current(generation)) this.reconnect();
    }, 800);
  },

  retryOpen() {
    if (this.openFailed) this.start(this.generation);
  },

  async loadModels(generation) {
    try {
      const response = await fetch(appUrl(`api/codex-sdk/models?${new URLSearchParams({uid: this.uid})}`));
      const data = await response.json();
      if (!this.current(generation) || !data?.ok) return;
      this.models = (data.raw?.data || []).filter(model => !model.hidden);
      this.paintPickers();
    } catch { /* 模型列表只影响选择器，发送仍使用会话当前设置 */ }
  },

  // ---------------------------------------------------------------- 用量
  async loadLimits(generation = this.generation) {
    if (!this.current(generation) || this.limitsRead) return;
    clearTimeout(this.limitsRetryTimer);
    this.limitsRetryTimer = 0;
    const read = new AbortController();
    this.limitsRead = read;
    try {
      const response = await fetch(appUrl(`api/codex-sdk/limits?${new URLSearchParams({uid: this.uid})}`), {signal: read.signal});
      const data = await response.json();
      if (!this.current(generation)) return;
      if (!response.ok || !data?.ok) throw new Error('limits unavailable');
      this.applyLimits(data.raw);
      this.limitsRetryDelay = 1000;
    } catch {
      // 保留上次成功值；短暂失败不能让用量一直隐藏到下一回合。
      if (this.current(generation)) {
        this.limitsRetryTimer = setTimeout(() => this.loadLimits(generation), this.limitsRetryDelay);
        this.limitsRetryDelay = Math.min(this.limitsRetryDelay * 2, 30000);
      }
    } finally {
      if (this.limitsRead === read) this.limitsRead = null;
    }
  },

  applyLimits(raw) {
    this.limits = raw?.rateLimitsByLimitId?.codex || raw?.rateLimits || null;
    this.limitsRecordedAt = null;
    this.limitsFresh = true;
    this.resetCards = raw?.rateLimitResetCredits || null;
    this.paintGauges();
  },

  seedGauges() {
    // The local rollout is already available; a slow/failed account request
    // must not hide its last recorded windows. Fresh account data always wins.
    const recorded = this.summary?.recorded_limits;
    if (!this.limitsFresh && !this.limits && recorded?.rateLimits) {
      this.limits = recorded.rateLimits;
      this.limitsRecordedAt = recorded.capturedAt || '';
    }
    this.paintGauges();
  },

  /** 与其它 agent 共用窗口名称。 */
  windowLabel(minutes) {
    return UsageUI.window(minutes);
  },

  gauge(slot, label, used, title) {
    const node = this.pane?.querySelector(`[data-gauge="${slot}"]`);
    if (!node) return;
    node.hidden = !Number.isFinite(used);
    if (node.hidden) return;
    const gauge=UsageUI.gauge(label,used,title);
    node.classList.toggle('low',gauge.classList.contains('low'));node.title=title;
    node.replaceChildren(...gauge.childNodes);
  },

  paintGauges() {
    if (!this.pane) return;
    const bucket = this.limits || {};
    const windows = [bucket.primary, bucket.secondary].filter(window => window && Number.isFinite(window.usedPercent));
    const slots = {'5h': null, '7d': null};
    const others = [];
    for (const window of windows) {
      const slot = {300:'5h',10080:'7d'}[window.windowDurationMins];
      if (slot) slots[slot] = window; else others.push(window);
    }
    // 官方窗口是 5h 与 7d；其它时长的窗口借用空闲的格子，不丢信息。
    for (const window of others) {
      const free = Object.keys(slots).find(key => !slots[key]);
      if (free) slots[free] = window;
    }
    for (const [slot, window] of Object.entries(slots)) {
      if (!window) { this.gauge(slot, slot, NaN, ''); continue; }
      const label = this.windowLabel(window.windowDurationMins);
      const reset = window.resetsAt ? `；${new Date(window.resetsAt * 1000).toLocaleString()} 重置` : '';
      const recorded = this.limitsRecordedAt != null
        ? `；上次记录${this.limitsRecordedAt ? ` ${new Date(this.limitsRecordedAt).toLocaleString()}` : ''}` : '';
      this.gauge(slot, label, window.usedPercent,
        `${label} 窗口已用 ${UsageUI.percent(window.usedPercent)}%，剩余 ${UsageUI.percent(Math.max(0,100-window.usedPercent))}%${reset}${recorded}`);
    }
    const usage = this.summary && Object.hasOwn(this.summary,"usage") ? this.summary.usage : this.cachedUsage;
    const size = usage?.modelContextWindow;
    const used = usage?.last?.totalTokens;
    if (size && used != null) {
      this.gauge('context', '上下文', used / size * 100,
        `上下文已用 ${Math.min(100, Math.round(used / size * 100))}%（${used.toLocaleString()} / ${size.toLocaleString()} tokens）`);
    } else this.gauge('context', '上下文', NaN, '');
    const credits = bucket.credits;
    const creditNode = this.pane.querySelector('.sdk-credits');
    const balance = Number(credits?.balance);
    creditNode.hidden = !credits || (!credits.unlimited && !credits.hasCredits && !Number.isFinite(balance));
    if (!creditNode.hidden) {
      creditNode.textContent = credits.unlimited ? '🪙 无限'
        : `🪙 ${Number.isFinite(balance) ? balance.toLocaleString(undefined, {maximumFractionDigits: 0}) : credits.balance}`;
      creditNode.title = '剩余 usage credits';
      creditNode.setAttribute('aria-label', creditNode.textContent.replace('🪙', 'Credits'));
    }
    const cards = this.resetCards?.credits || [];
    const holder = this.pane.querySelector('.sdk-reset');
    const select = holder.querySelector('select');
    const usable = cards.filter(row => !row.status || row.status === 'available');
    holder.hidden = !(this.resetCards?.availableCount > 0) || (cards.length > 0 && !usable.length);
    if (!holder.hidden) {
      const chosen = select.value;
      select.replaceChildren();
      const count = this.resetCards.availableCount;
      if (!cards.length) select.append(new Option(`重置卡 ×${count}`, ''));
      for (const card of usable) {
        const expires = card.expiresAt ? ` · ${new Date(card.expiresAt * 1000).toLocaleDateString(undefined, {year: '2-digit', month: 'numeric', day: 'numeric'})} 到期` : '';
        const option = new Option(`${card.title || '限额重置卡'}${expires}`, card.id);
        if (card.description) option.title = card.description;
        select.append(option);
      }
      if ([...select.options].some(option => option.value === chosen)) select.value = chosen;
      select.title = `共 ${count} 张可兑换`;
      holder.querySelector('#sdk-reset-use').textContent = `兑换 (${count})`;
    }
    this.layoutToolbar();
  },

  async redeem() {
    const select = this.pane?.querySelector('#sdk-reset-card');
    if (!select) return;
    const creditId = select.value, label = select.selectedOptions[0]?.textContent || '重置卡';
    this.closeToolbarMenus();
    const ok = await appConfirm(`兑换「${label}」？将重置对应的 Codex 限额窗口，且不可撤销。`, {ok: '兑换'});
    if (!ok || !this.pane) return;
    // 同一张卡的重试沿用同一个幂等键；成功后才换新键。
    const key = this.redeemKeys.get(creditId) || crypto.randomUUID();
    this.redeemKeys.set(creditId, key);
    const button = this.pane.querySelector('#sdk-reset-use');
    button.disabled = select.disabled = true;
    try {
      const data = await this.request('api/codex-sdk/reset-credit',
        {uid: this.uid, credit_id: creditId || null, idempotency_key: key});
      this.redeemKeys.delete(creditId);
      this.applyLimits(data.raw.limits);
      const outcome = data.raw.outcome?.outcome;
      this.setNotice({reset: '已兑换，限额窗口已重置。', alreadyRedeemed: '这张卡此前已兑换，已刷新用量。',
        nothingToReset: '当前没有可重置的限额窗口，未消耗卡片。', noCredit: '没有可用的重置卡。'}[outcome]
        || `兑换结果：${outcome || '未知'}`, outcome === 'reset' || outcome === 'alreadyRedeemed' ? '' : 'blocked');
      this.noticeHold = Date.now() + 8000;
    } catch (error) {
      this.setNotice(`兑换失败：${error.message}`, 'blocked');
      this.noticeHold = Date.now() + 8000;
    } finally {
      if (this.pane) button.disabled = select.disabled = false;
    }
  },

  paintPickers() {
    if (!this.pane) return;
    const modelSelect = this.pane.querySelector('#sdk-model');
    const currentModel = this.summary?.settings?.model || '';
    const modelName = this.modelChoice ?? currentModel;
    modelSelect.replaceChildren();
    for (const model of this.models) modelSelect.append(new Option(model.model, model.model));
    if (modelName && ![...modelSelect.options].some(option => option.value === modelName))
      modelSelect.append(new Option(modelName, modelName));
    this.selectValue(modelSelect, modelName, '选择模型');
    modelSelect.title = modelName;
    const select = this.pane.querySelector('#sdk-effort');
    const model = this.models.find(row => row.model === modelName);
    let chosen = this.effortChoice ?? this.summary?.settings?.effort ?? '';
    const switching = modelName !== currentModel;
    select.replaceChildren();
    for (const effort of model?.supportedReasoningEfforts || []) {
      const option = new Option(UsageUI.effort(effort.reasoningEffort), effort.reasoningEffort);
      option.title = effort.description || '';
      select.append(option);
    }
    if (chosen && ![...select.options].some(option => option.value === chosen)) {
      if (switching && this.effortChoice === null) chosen = '';
      else select.append(new Option(UsageUI.effort(chosen), chosen));
    }
    this.selectValue(select, chosen, '选择强度');
    CodexSdkControls.paintTier(this);
    this.layoutToolbar();
  },

  selectValue(select, value, label) {
    select.querySelector('option[value=""]')?.remove();
    if (value) {
      if (![...select.options].some(option => option.value === value)) select.append(new Option(value, value));
      select.value = value;
    }
    else {
      const placeholder = new Option(label, ''); placeholder.disabled = placeholder.hidden = true;
      select.prepend(placeholder); select.value = '';
    }
  },

  // ---------------------------------------------------------------- 历史
  async refreshHistory(generation, afterTurn = false) {
    if (this.pending(this.uid) && !afterTurn) return;
    try {
      if (this.historyReady) {
        const ready = this.historyReady;
        try { await ready; }
        catch (error) {
          if (this.historyReady !== ready) return;
          throw error;
        }
        finally { if (this.historyReady === ready) this.historyReady = null; }
        if (!this.current(generation)) return;
      }
      if (afterTurn && this.pendingAtOpen) {
        await loadSessions(true, true);
        if (!this.current(generation)) return;
        if (!this.pending(this.uid)) {
          this.pendingAtOpen = false;
          await openSession(this.uid, this.agent, {follow:true, historyMode:'replace'});
        }
      }
      // History is the same native head/tail window as the conversation view.
      // Catch up through its accepted live checkpoint; never fill the gap just
      // because an SDK view opened or a turn completed.
      const key = viewKey(this.uid, this.agent);
      if (afterTurn && cache.has(key)) await syncSession(this.uid, this.agent);
      if (!this.current(generation)) return;
      let cached = cacheGet(key);
      if (!cached) {
        const {data, bytes} = await fetchMessages(this.uid, {agent: this.agent, windowed: true});
        if (!this.current(generation)) return;
        cachePut(key, {meta: data.meta, msgs: data.messages, version: data.version,
          end: data.end, anchor: data.anchor, activity: data.activity, bytes,
          prompt: data.prompt || null, cli: data.cli ?? null,
          total: data.message_total, partial: data.partial || null});
        cached = cacheGet(key);
      }
      this.historyFailed = false;
      this.useHistory(cached, true);
    } catch (error) {
      if (this.current(generation)) {
        this.historyFailed = true;
        if (!this.historyData) this.history.innerHTML = `<div class="empty">读取历史失败：${esc(error.message || error)}</div>`;
        this.setNotice(`读取历史失败：${error.message || error}`, 'blocked');
      }
    }
  },

  useHistory(entry, force = false) {
    if (!this.pane || !entry) return;
    const data = this.historyData;
    const observed = observedConversationActivity(entry.activity, this.uid, this.agent).activity;
    if (!force && this.historyEntry === entry && data?.end === entry.end
        && data.messages === entry.msgs && data.length === entry.msgs.length
        && data.partial === entry.partial && data.activity === entry.activity) {
      // CLI-only updates can close a native turn whose ending was never
      // written. They must refresh status even when history is unchanged.
      if (this.summary && this.connected && (data.observedActivity?.state !== observed?.state
          || data.observedActivity?.reason !== observed?.reason)) this.paintStatus();
      data.observedActivity = observed;
      return;
    }
    this.historyEntry = entry;
    this.historyData = {messages: entry.msgs, partial: entry.partial,
      activity: entry.activity, observedActivity: observed, end: entry.end, length: entry.msgs.length};
    this.paintHistory();
  },

  paintHistory() {
    const data = this.historyData;
    if (!data || !this.history) return;
    const stick = this.nearBottom();
    let messages = data?.messages || [];
    let tailComplete = !this.turn && !['working', 'waiting'].includes(data?.activity?.state);
    const userText = this.activeUserText();
    const nativeStart = this.turn?.id ? messages.findIndex(message =>
      message.turn_id === this.turn.id && (userText || message.role !== 'user')) : -1;
    if (nativeStart >= 0) {
      messages = messages.slice(0, nativeStart);
      tailComplete = true;
    } else if (userText) {
      // 正在进行的回合已在 SDK 实时区显示；落盘历史截到它的用户消息之前。
      for (let i = messages.length - 1; i >= 0; i--) {
        if (messages[i].role === 'user' && String(messages[i].text || '').trim() === userText) {
          messages = messages.slice(0, i);
          tailComplete = true;
          break;
        }
      }
    }
    const folds = new Map([...this.history.querySelectorAll(':scope > .turn-process')]
      .map(node => [node.dataset.turnId, node.classList.contains('folded')]));
    this.history.replaceChildren();
    const partial = data.partial;
    const split = partial ? Math.min(partial.head, messages.length) : 0;
    const plan = partial
      ? [...planTurns(messages.slice(0, split).filter(sdkHistoryMessageVisible), {tailComplete: false, foldTail: true}),
         {gap: {uid: this.uid, agent: this.agent, omitted: partial.omitted}},
         ...planTurns(messages.slice(split).filter(sdkHistoryMessageVisible), {tailComplete, openTail: !tailComplete})]
      : planTurns(messages.filter(sdkHistoryMessageVisible), {tailComplete, openTail: !tailComplete});
    const built = buildPlan(this.history, plan, null);
    built.forEach((node, index) => CodexSdkActions.decorate(this, node, plan[index].m));
    for (const node of this.history.querySelectorAll(':scope > .turn-process')) {
      if (folds.has(node.dataset.turnId))
        folds.get(node.dataset.turnId) ? node._fold?.() : node._open?.();
    }
    refreshMessageTimeDividers(this.history);
    CodexSdkQuestions.paint(this);
    // 已经落盘的回合从实时区移除，避免同一回合出现两次。
    for (const [id, node] of this.nodes) {
      if (!this.items.has(id)) { node.remove(); this.nodes.delete(id); }
    }
    if (this.summary) this.paintStatus();
    if (stick) this.scrollBottom();
  },

  scheduleHistory() {
    clearTimeout(this.historyTimer);
    const generation = this.generation;
    this.historyTimer = setTimeout(() => this.refreshHistory(generation, true), 600);
  },

  activeUserText() {
    if (!this.turn) return '';
    for (const item of this.items.values()) {
      if (item.type === 'userMessage') return (item.content || []).filter(part => part.type === 'text').map(part => part.text || '').join('\n').trim();
    }
    return '';
  },

  // ---------------------------------------------------------------- 事件
  setConnected(connected) {
    this.connected = connected;
    if (!this.uid || this.agent) return;
    paintItemStatus(document.querySelector(`.item[data-uid="${CSS.escape(this.uid)}"]`));
    if (S.sel === this.uid) paintHeaderTurn();
  },

  applySnapshot(raw) {
    this.setConnected(true);
    // The SDK may attach between two identical global live snapshots.
    // Reconcile ownership on this connection's snapshot as well.
    queueUiChange({live: true});
    this.summary = raw.summary || {};
    this.seedGauges();
    this.turn = raw.turn || null;
    this.items = new Map((raw.items || []).map(item => [item.id, item]));
    this.live.replaceChildren();
    this.nodes.clear();
    this.requests = new Map((raw.requests || []).map(request => [request.key, request]));
    for (const item of this.items.values()) this.paintItem(item);
    for (const request of this.requests.values()) this.paintRequest(request);
    CodexSdkQuestions.paint(this);
    this.paintHistory();
    this.paintPickers();
    CodexSdkActions.paint(this);
    CodexSdkControls.paint(this);
    this.paintStatus();
    this.paintQueue();
    this.scrollBottom();
  },

  applyEvent(method, raw) {
    // Do not move a disclosure away from the pointer between press and release.
    const stick = this.nearBottom() && !this.box?.querySelector('.more:active');
    const item = raw.item;
    switch (method) {
      case 'bridge/stopped':
        clearTimeout(this.historyTimer); clearTimeout(this.reconnectTimer);
        this.es?.close(); this.es = null;
        this.turn = null; this.items.clear(); this.requests.clear(); this.live.replaceChildren();
        this.stopped = true; this.setConnected(false); this.paintStatus();
        queueUiChange({live: true});
        return;
      case 'bridge/queue':
        if (this.summary) this.summary.queue = raw.queue;
        this.paintQueue();
        break;
      case 'bridge/reset':
        this.setConnected(false);
        queueUiChange({live: true});
        this.setNotice('重连中', 'connecting');
        this.es?.close();
        this.scheduleReconnect();
        return;
      case 'thread/resumed': this.summary = raw.summary || this.summary; this.paintPickers(); break;
      case 'turn/started':
        this.turn = raw.turn; this.items.clear();
        if (this.summary) { this.summary.last_error = null; this.summary.retry_error = null; this.summary.last_turn = null; }
        break;
      case 'turn/completed':
        this.turn = null; this.items.clear();
        if (this.summary) {
          this.summary.last_turn = {id: raw.turn?.id, status: raw.turn?.status};
          this.summary.retry_error = null;
          if (raw.turn?.status === 'completed') this.summary.last_error = null;
        }
        if (raw.turn?.status === 'failed' && raw.turn.error && this.summary) this.summary.last_error = raw.turn.error;
        this.scheduleHistory();
        this.loadLimits();
        break;
      case 'thread/reverted':
        this.turn = null; this.items.clear(); this.requests.clear(); this.nodes.clear();
        this.live.replaceChildren();
        if (this.summary) { this.summary.usage = null; this.summary.last_error = null; this.summary.retry_error = null; this.summary.last_turn = null; }
        this.refreshHistory(this.generation, true);
        break;
      case 'item/started': case 'item/completed':
        if (this.summary) this.summary.retry_error = null;
        if (item?.type === 'contextCompaction')
          item.status = method === 'item/started' ? 'inProgress' : 'completed';
        if (item?.id) { this.items.set(item.id, item); this.paintItem(item); }
        break;
      case 'item/agentMessage/delta': case 'item/plan/delta':
        this.appendDelta(raw.itemId, 'text', raw.delta); break;
      case 'item/commandExecution/outputDelta':
        this.appendDelta(raw.itemId, 'aggregatedOutput', raw.delta); break;
      case 'item/reasoning/textDelta':
        if (this.summary) this.summary.retry_error = null;
        break;
      case 'item/reasoning/summaryTextDelta': {
        if (this.summary) this.summary.retry_error = null;
        const target = this.items.get(raw.itemId);
        if (target) {
          const parts = Array.isArray(target.summary) ? target.summary : (target.summary = []);
          const index = raw.summaryIndex || 0;
          while (parts.length <= index) parts.push('');
          parts[index] += raw.delta || '';
          this.paintItem(target);
        }
        break;
      }
      case 'serverRequest/resolved': {
        const key = JSON.stringify(raw.requestId);
        this.requests.delete(key);
        this.live.querySelector(`[data-sdk-request="${CSS.escape(key)}"]`)?.remove();
        break;
      }
      case 'thread/status/changed': if (this.summary) this.summary.status = raw.status; break;
      case 'thread/settings/updated': if (this.summary) this.summary.settings = raw.threadSettings; this.paintPickers(); break;
      case 'thread/name/updated':
        if (this.summary?.resume?.thread) this.summary.resume.thread.name = raw.threadName;
        void loadSessions(true, true);
        break;
      case 'thread/tokenUsage/updated': if (this.summary) this.summary.usage = raw.tokenUsage; break;
      case 'account/rateLimits/updated':
        // Rolling notifications are sparse: null means unavailable, including
        // account metadata and windows. Only a complete read removes a window.
        if (raw.rateLimits && (!this.limits || !raw.rateLimits.limitId || raw.rateLimits.limitId === this.limits.limitId)) {
          this.limits = mergeSdkLimits(this.limits, raw.rateLimits);
          this.limitsRecordedAt = null;
          this.limitsFresh = true;
        }
        break;
      case 'error':
        if (this.summary) {
          this.summary.retry_error = raw.willRetry === true ? raw.error : null;
          this.summary.last_error = raw.willRetry === true ? null : raw.error;
        }
        break;
      default:
        if (SDK_REQUEST_METHODS.has(method)) {
          const request = {key: raw.key, method, params: raw.params || {}};
          this.requests.set(request.key, request);
          this.paintRequest(request);
        }
    }
    CodexSdkQuestions.paint(this);
    CodexSdkActions.paint(this);
    CodexSdkControls.paint(this);
    this.paintStatus();
    if (stick) this.scrollBottom();
  },

  appendDelta(id, field, delta) {
    if (this.summary) this.summary.retry_error = null;
    const target = this.items.get(id);
    if (!target) return;
    target[field] = (target[field] || '') + (delta || '');
    this.paintItem(target);
  },

  // ---------------------------------------------------------------- 渲染
  paintItem(item) {
    const message = item.questions?.length ? null : itemMessage(item);
    const old = this.nodes.get(item.id);
    if (!message) { old?.remove(); this.nodes.delete(item.id); return; }
    if (old?._updateMessage && old.dataset.role === message.role) {
      old._updateMessage(message);
      return;
    }
    const toolControls = ['.tool-out-actions .more', '.tool-toggle', '.tool-wrap'];
    const toolState = TOOL_ROLES.has(message.role) && old
      ? toolControls.map(selector => [...old.querySelectorAll(selector)].map(control =>
          control.getAttribute(selector === '.tool-wrap' ? 'aria-pressed' : 'aria-expanded')))
      : null;
    const node = msgNode(message);
    node.dataset.sdkItem = item.id;
    CodexSdkActions.decorate(this, node, {...message, turn_id: this.turn?.id});
    if (item.status === 'inProgress' && TOOL_ROLES.has(message.role)) {
      const status = document.createElement('div');
      status.className = 'tool-status';
      status.textContent = '运行中…';
      node.querySelector('.tool-entry')?.append(status);
    }
    if (old) old.replaceWith(node);
    else {
      const firstRequest = this.live.querySelector('[data-sdk-request]');
      this.live.insertBefore(node, firstRequest);
    }
    if (toolState) toolControls.forEach((selector, group) => {
      node.querySelectorAll(selector).forEach((control, index) => {
        const attribute = selector === '.tool-wrap' ? 'aria-pressed' : 'aria-expanded';
        const previous = toolState[group][index];
        if (previous != null && control.getAttribute(attribute) !== previous) control.click();
      });
    });
    this.nodes.set(item.id, node);
  },

  paintRequest(request) {
    const old = this.live.querySelector(`[data-sdk-request="${CSS.escape(request.key)}"]`);
    const card = requestCard(request, result => this.answer(request, result, card));
    card.dataset.sdkRequest = request.key;
    if (old) old.replaceWith(card); else this.live.append(card);
  },

  paintStatus() {
    if (!this.pane) return;
    if (this.stopped) {
      this.pane.querySelector('#sdk-send').disabled = true;
      this.pane.querySelector('#sdk-stop').hidden = true;
      this.setNotice('会话已停止；重新打开 SDK 可继续。', 'idle', true);
      return;
    }
    const active = !!this.turn;
    this.pane.querySelector('#sdk-stop').hidden = !active;
    const send = this.pane.querySelector('#sdk-send');
    send.textContent = active ? '排队' : '发送';
    send.title = active ? '当前回合结束后发送；排队消息可撤销或立即引导' : '开始新回合';
    this.paintQueue();
    this.paintGauges();
    // 兑换结果提示停留几秒，不被紧随其后的状态刷新盖掉。
    if (this.noticeHold > Date.now()) return;
    const status = this.summary?.status || {};
    const observed = observedConversationActivity(this.historyData?.activity, this.uid, this.agent);
    // Idle describes the daemon's availability, not the preceding turn's
    // outcome. Prefer an observed native completion over older disk history.
    const interrupted = !active && (this.summary?.last_turn?.status === 'interrupted'
      || (!this.summary?.last_turn && observed.activity?.state === 'aborted'));
    const compact = [...this.items.values()].findLast(item => item.type === 'contextCompaction');
    const parts = [];
    let kind = 'ready';
    if (this.limits?.rateLimitReachedType) { parts.push('用量已满'); kind = 'waiting'; }
    const nativeError = !active && status.type === 'systemError'
      && this.historyData?.activity?.state === 'failed'
      ? this.historyData.activity.error : null;
    const errorText = this.summary?.last_error?.message || nativeError;
    if (errorText) { parts.push(`错误：${errorText}`); kind = 'blocked'; }
    else if (this.summary?.retry_error?.message) { parts.push(`正在重试：${this.summary.retry_error.message}`); kind = 'retrying'; }
    if (!parts.length) {
      if (status.type === 'systemError') { parts.push('系统错误'); kind = 'blocked'; }
      else if (status.activeFlags?.includes('waitingOnApproval')) { parts.push('待批准'); kind = 'waiting'; }
      else if (status.activeFlags?.includes('waitingOnUserInput')) { parts.push('待回答'); kind = 'waiting'; }
      else if (compact?.status === 'inProgress') { parts.push('压缩中'); kind = 'maintenance'; }
      else if (active) { parts.push('处理中'); kind = 'working'; }
      else if (interrupted) {
        parts.push('已中断' + (!this.summary?.last_turn && observed.activity?.reason ? '：' + observed.activity.reason : ''));
        kind = 'warning';
      }
      else if (compact?.status === 'completed') parts.push('压缩完成');
      else parts.push('就绪');
    }
    this.setNotice(parts.join(' '), this.summary?.last_error || status.type === 'systemError' ? 'blocked' : kind);
  },

  setNotice(text, kind = '', sticky = false) {
    if (!this.notice) return;
    this.notice.firstElementChild.textContent = sdkStatusSummary(text, kind);
    const details = this.pane.querySelector('#sdk-status-menu');
    details.textContent = text;
    if (!text && details.matches(':popover-open')) details.hidePopover();
    this.notice.title = text;
    this.notice.hidden = !text;
    this.notice.className = 'sdk-status' + (kind ? ` ${kind}` : '');
    this.notice.dataset.state = kind || 'neutral';
    this.notice.dataset.sticky = sticky ? '1' : '';
    this.updateStatusDisclosure();
  },

  updateStatusDisclosure() {
    if (!this.notice) return;
    sdkUpdateStatusDisclosure(this.notice, this.pane.querySelector('#sdk-status-menu'));
  },

  nearBottom() {
    const box = this.box;
    return !box || box.scrollHeight - box.scrollTop - box.clientHeight < 80;
  },

  scrollBottom() {
    if (this.box) this.box.scrollTop = this.box.scrollHeight;
  },

  fitInput() {
    const input = this.input;
    if (!input) return;
    input.classList.toggle('shell-mode', /^\s*(?:!|\/shell(?:\s|$))/.test(input.value));
    input.style.height = 'auto';
    input.style.height = Math.min(input.scrollHeight, 240) + 'px';
    input.style.overflowY = input.scrollHeight > 240 ? 'auto' : 'hidden';
  },

  // ---------------------------------------------------------------- 操作
  paintQueue() {
    const holder = this.pane?.querySelector('.sdk-queue');
    if (!holder) return;
    // Status/usage deltas must not replace the button between pointerdown/up.
    const contents = JSON.stringify([!!this.turn, this.summary?.queue || []]);
    if (holder._sdkQueueBusy || holder._sdkQueueContents === contents) return;
    holder._sdkQueueContents = contents;
    holder.replaceChildren();
    for (const item of this.summary?.queue || []) {
      const card = document.createElement('div');
      card.className = 'msg queued-send sdk-queued'; card.dataset.role = 'user'; card.dataset.queueId = item.id;
      const text = document.createElement('div'); text.className = 'mb'; text.textContent = item.text;
      const foot = document.createElement('div'); foot.className = 'queued-send-foot';
      const status = document.createElement('span'); status.className = 'queued-send-state';
      status.textContent = item.state === 'failed' ? (item.error || '发送失败') : item.state === 'sending' ? '正在发送…' : '已排队，当前回合结束后发送';
      const controls = document.createElement('span'); controls.className = 'sdk-queue-actions';
      foot.append(status, controls);
      for (const [action, label] of [['cancel', '撤销'], [item.state === 'failed' ? 'retry' : 'steer', item.state === 'failed' ? '重试' : '立即引导']]) {
        const button = document.createElement('button');
        button.type = 'button'; button.className = 'queued-send-dismiss';
        button.dataset.queueAction = action; button.textContent = label;
        button.title = action === 'steer' ? '将这条消息立即引导到正在进行的回合（steer）' : label;
        button.disabled = item.state === 'sending' || (action === 'steer' && !this.turn);
        button.onclick = async () => {
          const generation = this.generation;
          holder._sdkQueueBusy = true;
          for (const node of foot.querySelectorAll('button')) node.disabled = true;
          try {
            await this.request('api/codex-sdk/queue', {uid: this.uid, id: item.id, action});
            if (action === 'cancel' && this.current(generation)) {
              const state = CodexSdkCompose.states.get(this), holder = state?.draft || state;
              if (holder?.sdkSubmission?.id === item.id) {
                delete holder.sdkSubmission;
                if (state.draft) void persistComposerDraft(this.uid);
              }
            }
          }
          catch (error) { if (this.current(generation)) this.setNotice(`队列操作失败：${error.message}`, 'blocked', true); }
          finally {
            holder._sdkQueueBusy = false;
            holder._sdkQueueContents = null;
            if (this.current(generation)) this.paintQueue();
          }
        };
        controls.append(button);
      }
      card.append(text, foot); holder.append(card);
    }
  },

  async send() {
    const input = this.input;
    if (!input || !this.pane) return;
    const generation = this.generation;
    const button = this.pane.querySelector('#sdk-send');
    if (button.disabled) return;
    button.disabled = true;
    button.setAttribute('aria-busy', 'true');
    const model = this.modelChoice, effort = this.effortChoice;
    try {
      const payload = await CodexSdkCompose.collect(this);
      if (!payload || !this.current(generation)) return;
      const text = payload.text;
      if (await this.command(text)) {
        return;
      }
      if (text.trimStart().startsWith('/') || text.trimStart().startsWith('!')) {
        this.setNotice('不支持的命令。输入 /help 查看可用命令；草稿已保留。', 'blocked');
        return;
      }
      const request_id = await CodexSdkCompose.submission(payload, model, effort, this.agent);
      const response = await this.request('api/codex-sdk/turn', {
        uid: payload.uid, text, attachments: payload.attachments, quotes: payload.quotes,
        model, effort, request_id,
      });
      if (!await CodexSdkCompose.accepted(this, payload, response, request_id)) return;
      if (this.input === input) {
        if (!response.raw?.steered) {
          if (this.modelChoice === model) this.modelChoice = null;
          if (this.effortChoice === effort) this.effortChoice = null;
        }
        this.paintPickers();
      }
      await CodexSdkCompose.sent(this, payload);
    } catch (error) {
      if (this.current(generation)) this.setNotice(`发送失败：${error.message}`, 'blocked');
    } finally {
      if (this.current(generation)) CodexSdkCompose.failed(this);
      button.disabled = false;
      button.removeAttribute('aria-busy');
    }
  },

  async command(text) {
    if (await CodexSdkActions.command(this, text) || await CodexSdkControls.command(this, text)) return true;
    const match = text.trim().match(/^\/(help|model|effort|status|stop|new)(?:\s+([\s\S]*))?$/);
    if (!match) return false;
    const [, command, value] = match;
    switch (command) {
      case 'help': this.setNotice('可用命令：/model [模型]、/effort [强度]、/plan、/default、/permissions、/shell 命令（或 !命令）、/rename [名称]、/fork、/revert、/compact、/review、/status、/stop、/new。'); break;
      case 'model': case 'effort': {
        const select = this.pane.querySelector(`#sdk-${command}`);
        if (value) {
          if (![...select.options].some(option => option.value === value)) throw new Error(`${command === 'model' ? '模型' : '推理强度'}不在可选列表中`);
          select.value = value; select.dispatchEvent(new Event('change'));
        } else select.focus();
        break;
      }
      case 'status': this.setNotice(`模型：${this.summary?.settings?.model || '未返回'}；推理强度：${this.summary?.settings?.effort || '未返回'}；${this.turn ? '运行中' : '空闲'}`); break;
      case 'stop': await this.request('api/codex-sdk/interrupt', {uid: this.uid}); break;
      case 'new': document.querySelector('#new-session')?.click(); break;
    }
    if (this.input?.value === text) { this.input.value = ''; this.fitInput(); CodexSdkCompose.input(this); }
    return true;
  },

  async interrupt() {
    try {
      await this.request('api/codex-sdk/interrupt', {uid: this.uid});
    } catch (error) {
      this.setNotice(`中断失败：${error.message}`, 'blocked');
    }
  },

  async answer(request, result, card) {
    card.querySelectorAll('button, input, textarea').forEach(control => { control.disabled = true; });
    try {
      await this.request('api/codex-sdk/respond', {uid: this.uid, key: request.key, result});
      card.classList.add('sdk-answered');
    } catch (error) {
      card.querySelectorAll('button, input, textarea').forEach(control => { control.disabled = false; });
      this.setNotice(`回答失败：${error.message}`, 'blocked');
    }
  },
};

const SDK_REQUEST_METHODS = new Set([
  'item/commandExecution/requestApproval', 'item/fileChange/requestApproval',
  'item/tool/requestUserInput', 'item/permissions/requestApproval', 'mcpServer/elicitation/request',
]);

function userMessageText(item) {
  return (item.content || []).map(part => {
    if (part.type === 'text') return part.text || '';
    if (part.type === 'image') return `[图片] ${part.url || ''}`;
    if (part.type === 'localImage') return `[图片] ${part.path || ''}`;
    if (part.type === 'skill' || part.type === 'mention') return '';
    return '';
  }).filter(Boolean).join('\n');
}

const SDK_JSON = value => typeof value === 'string' ? value : JSON.stringify(value ?? null, null, 2);

function mergeSdkLimits(previous, update) {
  const merged = {...previous};
  for (const [key, value] of Object.entries(update)) {
    if (value == null) continue;
    merged[key] = typeof value === 'object' && !Array.isArray(value)
      ? mergeSdkLimits(previous?.[key], value) : value;
  }
  return merged;
}

/** app-server ThreadItem → 消息模式的消息对象，由同一个 msgNode 渲染。 */
function sdkHistoryMessageVisible(message) { return message.event_kind !== 'compact'; }

function itemMessage(item) {
  const done = ['completed', 'failed', 'declined'].includes(item.status);
  const failed = item.status === 'failed' || item.status === 'declined';
  switch (item.type) {
    case 'userMessage': return {role: 'user', text: userMessageText(item)};
    case 'agentMessage': return item.text ? {role: 'assistant', text: item.text, phase: item.phase} : null;
    case 'plan': return item.text ? {role: 'assistant', text: item.text} : null;
    case 'reasoning': {
      const text = (item.summary || []).filter(Boolean).join('\n\n');
      return text ? {role: 'thinking', text} : null;
    }
    case 'commandExecution': {
      const command = item.commandActions?.[0]?.command || item.command || '';
      const output = item.aggregatedOutput || '';
      return {role: 'tool', name: 'shell', summary: `$ ${command}`, text: item.command || command,
        result: done || output ? {role: 'tool_result', text: output, exit_code: item.exitCode ?? undefined,
          duration_s: item.durationMs != null ? item.durationMs / 1000 : undefined,
          error: failed || (item.exitCode != null && item.exitCode !== 0)} : null};
    }
    case 'fileChange': {
      const changes = item.changes || [];
      const kind = change => typeof change.kind === 'string' ? change.kind : change.kind?.type || '';
      return {role: 'tool', name: 'apply_patch', summary: `修改 ${changes.length} 个文件`,
        text: changes.map(change => `${kind(change)} ${change.path}`.trim()).join('\n'),
        result: done ? {role: 'tool_result', text: changes.map(change => change.diff || '').join('\n'),
          error: failed} : null};
    }
    case 'mcpToolCall':
      return {role: 'tool', name: `${item.server}.${item.tool}`, summary: `${item.server} · ${item.tool}`,
        text: SDK_JSON(item.arguments),
        result: done ? {role: 'tool_result', text: item.error ? SDK_JSON(item.error) : SDK_JSON(item.result),
          error: !!item.error || failed} : null};
    case 'dynamicToolCall':
      return {role: 'tool', name: item.tool, summary: item.tool, text: SDK_JSON(item.arguments),
        result: done ? {role: 'tool_result', text: SDK_JSON(item.contentItems), error: item.success === false || failed} : null};
    case 'collabAgentToolCall':
      return {role: 'tool', name: item.tool, summary: `${item.tool}${item.prompt ? ' · ' + item.prompt : ''}`,
        text: SDK_JSON(item), result: done || item.status === 'interrupted'
          ? {role: 'tool_result', text: SDK_JSON(item.agentsStates || {}), error: failed || item.status === 'interrupted'} : null};
    case 'webSearch':
      return {role: 'tool', name: 'web_search', summary: `搜索 ${item.query || item.action?.query || item.action?.url || ''}`,
        text: SDK_JSON(item.action || item.query)};
    case 'imageView': return {role: 'tool', name: 'view_image', summary: `查看图片 ${item.path || ''}`, text: item.path || ''};
    case 'enteredReviewMode': return {role: 'assistant', text: `开始审查：${item.review || ''}`};
    case 'exitedReviewMode': return item.review ? {role: 'assistant', text: item.review} : null;
    case 'contextCompaction': return null;
    case 'functionCallOutput':
      return {role: 'tool_result', name: item.name, text: SDK_JSON(item.output)};
    default: return {role: 'tool', name: item.type, summary: item.type, text: SDK_JSON(item)};
  }
}

const SDK_DECISIONS = {
  accept: '批准', acceptForSession: '本会话内批准', decline: '拒绝', cancel: '拒绝并中断',
};

function sdkDecisionLabel(decision) {
  if (typeof decision === 'string') return SDK_DECISIONS[decision] || decision;
  if (decision?.acceptWithExecpolicyAmendment) return '批准并记住此命令';
  const network = decision?.applyNetworkPolicyAmendment?.network_policy_amendment;
  if (network) return `${network.action === 'deny' ? '始终拒绝' : '始终允许'} ${network.host}`;
  return JSON.stringify(decision);
}

/** 服务端请求的题卡：审批、提问、权限、MCP 表单。按钮回传原生结构。 */
function requestCard(request, submit) {
  const params = request.params || {};
  const card = document.createElement('div');
  card.className = 'msg sdk-request';
  card.dataset.role = 'question';
  const body = document.createElement('div');
  body.className = 'mb';
  card.append(body);
  const line = (text, cls = '') => {
    if (!text) return;
    const p = document.createElement('p');
    if (cls) p.className = cls;
    p.textContent = text;
    body.append(p);
  };
  const pre = text => {
    if (!text) return;
    const block = document.createElement('pre');
    block.textContent = text;
    body.append(block);
  };
  const actions = document.createElement('div');
  actions.className = 'sdk-request-actions';
  const button = (label, result, go = false) => {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'btn' + (go ? ' go' : '');
    b.textContent = label;
    b.onclick = () => {
      const value = typeof result === 'function' ? result() : result;
      if (value != null) submit(value);
    };
    actions.append(b);
    return b;
  };
  switch (request.method) {
    case 'item/commandExecution/requestApproval': {
      const network = params.networkApprovalContext;
      line(network ? `需要批准网络访问：${network.protocol || ''} ${network.host || ''}` : '需要批准执行命令', 'sdk-request-title');
      if (!network) pre(params.command);
      line(params.cwd ? `目录：${params.cwd}` : '');
      line(params.reason);
      const decisions = params.availableDecisions || ['accept', 'acceptForSession', 'decline', 'cancel'];
      decisions.forEach((decision, index) => button(sdkDecisionLabel(decision), {decision}, index === 0));
      break;
    }
    case 'item/fileChange/requestApproval':
      line('需要批准文件修改', 'sdk-request-title');
      line(params.reason);
      line(params.grantRoot ? `授权目录：${params.grantRoot}` : '');
      ['accept', 'acceptForSession', 'decline', 'cancel']
        .forEach((decision, index) => button(SDK_DECISIONS[decision], {decision}, index === 0));
      break;
    case 'item/permissions/requestApproval':
      line('需要授予额外权限', 'sdk-request-title');
      line(params.reason);
      pre(SDK_JSON(params.permissions));
      button('授予（本回合）', {permissions: params.permissions || {}, scope: 'turn'}, true);
      button('授予（本会话）', {permissions: params.permissions || {}, scope: 'session'});
      button('拒绝', {permissions: {}, scope: 'turn'});
      break;
    case 'item/tool/requestUserInput': {
      line('Codex 需要你回答', 'sdk-request-title');
      const readers = [];
      for (const question of params.questions || []) {
        const group = document.createElement('fieldset');
        group.className = 'sdk-question';
        const legend = document.createElement('legend');
        legend.textContent = [question.header, question.question].filter(Boolean).join(' · ');
        group.append(legend);
        const name = `sdk-q-${request.key}-${question.id}`;
        for (const option of question.options || []) {
          const label = document.createElement('label');
          const radio = document.createElement('input');
          radio.type = 'radio'; radio.name = name; radio.value = option.label;
          label.append(radio, ` ${option.label}`);
          if (option.description) label.title = option.description;
          group.append(label);
        }
        let other = null;
        if (question.isOther || !(question.options || []).length) {
          other = document.createElement('input');
          other.type = question.isSecret ? 'password' : 'text';
          other.placeholder = (question.options || []).length ? '其他答案' : '输入答案';
          group.append(other);
        }
        body.append(group);
        readers.push(() => {
          const picked = group.querySelector('input[type=radio]:checked')?.value;
          const typed = other?.value.trim();
          return [question.id, {answers: typed ? [typed] : picked ? [picked] : []}];
        });
      }
      button('提交', () => ({answers: Object.fromEntries(readers.map(read => read()))}), true);
      break;
    }
    case 'mcpServer/elicitation/request': {
      line(`${params.serverName || 'MCP'} 请求确认`, 'sdk-request-title');
      line(params.message);
      if (params.url) {
        const link = document.createElement('a');
        link.href = params.url; link.target = '_blank'; link.rel = 'noopener';
        link.textContent = params.url;
        body.append(link);
        button('已完成', {action: 'accept', content: null}, true);
      } else {
        pre(SDK_JSON(params.requestedSchema));
        const input = document.createElement('textarea');
        input.className = 'sdk-json';
        input.placeholder = '按上面的结构填写 JSON';
        input.value = '{}';
        body.append(input);
        const error = document.createElement('p');
        error.className = 'sdk-control-error'; error.setAttribute('role', 'alert'); error.hidden = true;
        body.append(error);
        button('提交', () => {
          try {
            const content = JSON.parse(input.value);
            error.hidden = true; input.removeAttribute('aria-invalid');
            return {action: 'accept', content};
          } catch (failure) {
            error.textContent = `JSON 格式无效：${failure.message}`; error.hidden = false;
            input.setAttribute('aria-invalid', 'true'); input.focus();
            return null;
          }
        }, true);
      }
      button('拒绝', {action: 'decline', content: null});
      button('取消', {action: 'cancel', content: null});
      break;
    }
    default:
      line(request.method);
      pre(SDK_JSON(params));
  }
  card.append(actions);
  return card;
}


// Async questions arrive as agentMessage.questions, not server requests. Their
// answer is an ordinary user message; explicit submission may steer a live turn.
const CodexSdkQuestions = {
  answered: new Set(),

  paint(view) {
    if (!view.live) return;
    const pending = new Map();
    let lastUser = '';
    for (const message of view.historyData?.messages || []) {
      if (message.role === 'user') { pending.clear(); lastUser = String(message.text || '').trim(); }
      if (message.role !== 'tool' || !/(^|[._])request_user_input_async$/.test(message.name || '')) continue;
      try {
        const questions = JSON.parse(message.text).questions;
        if (questions?.length) pending.set(message.call_id, questions);
      } catch (_) { /* A malformed historical tool call is still shown as a tool. */ }
    }
    for (const item of view.items.values()) {
      if (item.type === 'userMessage') {
        const text = userMessageText(item).trim();
        if (text !== lastUser) pending.clear();
        lastUser = text;
      }
      if (item.type === 'agentMessage' && item.questions?.length) pending.set(item.id, item.questions);
    }
    const scope = JSON.stringify([view.uid, view.agent]);
    for (const card of view.live.querySelectorAll('[data-sdk-async-question]')) {
      if (!pending.has(card.dataset.sdkAsyncQuestion)) card.remove();
    }
    for (const [id, questions] of pending) {
      if (!id) continue;
      const requestId = `sdk-question:${id}`;
      const key = `${scope}:${id}`;
      if (this.answered.has(key) || (view.summary?.queue || []).some(row => row.id === requestId)) continue;
      if (view.live.querySelector(`[data-sdk-async-question="${CSS.escape(id)}"]`)) continue;
      const request = {key: id, method: 'item/tool/requestUserInput', params: {questions: questions.map((q, i) => ({
        id: String(i), question: q.title, isOther: true,
        options: (q.options || []).map(label => ({label})),
      }))}};
      const card = requestCard(request, async result => {
        const answers = questions.map((q, i) => result.answers[String(i)]?.answers?.[0] || '');
        if (answers.some(answer => !answer)) {
          view.setNotice('请回答每个问题后提交。', 'blocked');
          return;
        }
        const generation = view.generation;
        const text = questions.map((q, i) => `${q.title}\n• ${answers[i]}`).join('\n\n');
        card.querySelectorAll('button, input').forEach(control => { control.disabled = true; });
        try {
          const response = await view.request('api/codex-sdk/turn', {uid: view.uid, request_id: requestId, text});
          this.answered.add(key);
          card.classList.add('sdk-answered');
          card.querySelector('button').textContent = '已提交';
          if (view.current(generation) && view.turn && response.raw?.state === 'queued') {
            await view.request('api/codex-sdk/queue', {uid: view.uid, id: requestId, action: 'steer'});
          }
        } catch (error) {
          if (!view.current(generation)) return;
          if (this.answered.has(key)) {
            view.setNotice(`回答已保存到队列，立即引导失败：${error.message}。可在队列中重试。`, 'blocked');
          } else {
            card.querySelectorAll('button, input').forEach(control => { control.disabled = false; });
            view.setNotice(`回答失败：${error.message}`, 'blocked');
          }
        }
      });
      card.dataset.sdkAsyncQuestion = id;
      card.querySelector('button').textContent = '提交回答';
      view.live.append(card);
    }
  },
};

// A complete short status is a label, not an invitation to repeat it in a menu.
function sdkUpdateStatusDisclosure(notice, details) {
  const full = details.textContent.trim(), summary = notice.textContent.trim();
  const available = !notice.hidden && !!full
    && (full !== summary || notice.scrollWidth > notice.clientWidth + 1);
  notice.disabled = !available;
  if (available) {
    notice.setAttribute('aria-controls', details.id);
    notice.setAttribute('aria-haspopup', 'dialog');
    notice.setAttribute('aria-expanded', String(details.matches(':popover-open')));
  } else {
    if (details.matches(':popover-open')) details.hidePopover();
    for (const attribute of ['aria-controls', 'aria-haspopup', 'aria-expanded']) notice.removeAttribute(attribute);
  }
}

// Keep protocol detail intact; only the toolbar label is summarized.
function sdkStatusSummary(text, kind) {
  text = String(text || '');
  if ([...text].length <= 6) return text;
  const lead = text.split(/[：:，,。；;\s]/)[0];
  if (lead && [...lead].length <= 6) return lead;
  if (text.startsWith('不支持的命令')) return '命令不支持';
  if (text.startsWith('队列暂停')) return '队列暂停';
  return {blocked: '执行失败', retrying: '正在重试', working: '处理中',
    waiting: '等待处理', maintenance: '压缩中', connecting: '连接中', ready: '就绪'}[kind] || '状态详情';
}

function sdkLayoutFooter(view, controls) {
  const row = view.pane.querySelector('.sdk-status-row'), gauges = row.querySelector('.sdk-gauges');
  const pickers = row.querySelector('.sdk-pickers'), menu = view.pane.querySelector('#sdk-more-menu');
  const trigger = view.pane.querySelector('#sdk-more');
  const signature = JSON.stringify([row.clientWidth, view.notice.textContent, gauges.textContent,
    [...menu.querySelectorAll('.sdk-control-note, .sdk-reset')].map(n => [n.hidden, n.textContent]),
    controls.filter(Boolean).map(n => [n.hidden, n.textContent, n.querySelector('select')?.value])]);
  if (row.dataset.footerLayout === signature) return;
  row.dataset.footerLayout = signature;
  gauges.tabIndex = 0;
  if (row.dataset.footerWidth && Number(row.dataset.footerWidth) !== row.clientWidth) view.closeToolbarMenus();
  row.dataset.footerWidth = row.clientWidth;
  // Start with status, usage and ⋯; optional controls never displace usage.
  trigger.hidden = false;
  const overflow = control => control.classList.contains('sdk-action') ? view.pane.querySelector('#sdk-actions') : menu;
  for (const control of controls.filter(Boolean)) overflow(control).append(control);
  const fits = () => row.scrollWidth <= row.clientWidth + 1 && gauges.scrollWidth <= gauges.clientWidth + 1;
  const anchor = trigger;
  for (const control of controls.filter(Boolean)) {
    pickers.insertBefore(control, anchor);
    if (!fits()) { overflow(control).append(control); break; }
  }
  const overflowPickers=controls.filter(control=>control?.matches('.new-effort') && control.parentElement===menu);
  menu.prepend(...overflowPickers);
  for(const control of controls.filter(control=>control?.matches('.new-effort'))){
    const inMenu=control.parentElement===menu,select=control.querySelector('select');
    let label=control.querySelector(':scope > .visually-hidden, :scope > .sdk-picker-label');
    if(!label && select){label=el('span','',select.getAttribute('aria-label') || control.title);control.prepend(label);}
    label?.classList.toggle('visually-hidden',!inMenu);label?.classList.toggle('sdk-picker-label',inMenu);
  }
  // Empty wrappers and hidden reset cards are not menu contents. The same
  // contents decide both disclosure visibility and boundaries between groups.
  const hasContent = node => !node.hidden && (node.matches('button, input, select, textarea')
    || [...node.childNodes].some(child => child.nodeType === Node.TEXT_NODE
      ? !!child.textContent.trim() : child.nodeType === Node.ELEMENT_NODE && hasContent(child)));
  for (const group of menu.querySelectorAll(':scope > #sdk-actions, :scope > #sdk-permissions')) {
    group.hidden = false;
    group.hidden = !hasContent(group);
  }
  trigger.hidden = !hasContent(menu);
  if (trigger.hidden && menu.matches(':popover-open')) {
    menu.hidePopover();
    view.input?.focus();
  }
}
