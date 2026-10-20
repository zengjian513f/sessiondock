'use strict';

// Source-neutral SDK view contract. Providers own snapshots, status semantics,
// settings and protocol actions; history uses the ordinary conversation cache.
const SdkUI = {
  mount(view, {name, readOnly, historyReady, settings}) {
    const pane = view.pane;
    pane.setAttribute('aria-label', `${name} SDK`);
    pane.classList.add('sdk-pane');
    pane.innerHTML = `<div class="sdk-view">
      <div class="msgs sdk-msgs" aria-live="polite"><div class="sdk-history"></div><div class="sdk-live"></div><div class="sdk-queue"></div></div>
      <div id="sdk-composer">
        <div class="sdk-status-row">
          <button class="sdk-status" data-state="${readOnly ? 'idle' : 'connecting'}" type="button" disabled><span role="status" aria-live="polite">${readOnly ? '未运行' : '连接中'}</span></button>
          <div class="new-model-menu sdk-toolbar-menu sdk-status-menu" id="sdk-status-menu" popover="auto" role="dialog" aria-label="完整状态"></div>
          <div class="sdk-gauges" aria-label="用量"></div>
          <div class="sdk-options"><div class="sdk-pickers">
            <label class="new-effort sdk-model sdk-model-fit" title="模型" hidden><span class="visually-hidden">模型</span><select id="sdk-model" aria-label="模型"></select></label>
            <label class="new-effort sdk-effort" title="推理强度" hidden><span class="visually-hidden">推理强度</span><select id="sdk-effort" aria-label="推理强度"></select></label>
            <button class="btn" id="sdk-more" type="button" title="会话操作" aria-label="会话操作" aria-expanded="false" aria-controls="sdk-more-menu">⋯</button>
            <div class="new-model-menu sdk-toolbar-menu" id="sdk-more-menu" popover="auto" aria-label="会话操作"></div>
          </div></div>
        </div>
        <div class="composer-row">
          <div class="composer-input-wrap"><textarea id="sdk-input" rows="1" enterkeyhint="enter" aria-label="消息" placeholder="通过 ${esc(name)} SDK 发送"></textarea></div>
          <div class="cbtns"><button class="btn" id="sdk-stop" type="button" hidden title="中断当前回合">停止</button><button class="btn go" id="sdk-send" type="button">发送</button></div>
        </div>
      </div></div>`;
    view.box = pane.querySelector('.sdk-msgs');
    observeMessageLayout(view.box);
    view.history = pane.querySelector('.sdk-history');
    if (historyReady) view.history.innerHTML = '<div class="spin">正在读取会话…</div>';
    view.live = pane.querySelector('.sdk-live');
    view.notice = pane.querySelector('.sdk-status');
    view.input = pane.querySelector('#sdk-input');
    pane.querySelector('#sdk-composer').hidden = false;
    view.input.disabled = true;
    pane.querySelector('#sdk-send').disabled = true;
    view.input.onclick = () => startComposerSession(view.uid, view.agent, 'sdk');
    view.input.oninput = () => { view.fitInput(); CodexSdkCompose.input(view); };
    view.input.onblur = () => PathCompletion.close(view.input);
    view.input.onkeydown = event => {
      if (PathCompletion.key(view.input, event, () => view.uid)) return;
      ConversationUI.submitOnEnter(event, () => view.send());
    };
    pane.querySelector('#sdk-send').onclick = () => view.send();
    pane.querySelector('#sdk-stop').onclick = () => view.interrupt();
    // Match the conversation editor: clicking SEND must not leave focus on a
    // disabled button while the request is pending.
    pane.querySelector('#sdk-send').onmousedown = event => event.preventDefault();
    for (const kind of ['model', 'effort']) {
      pane.querySelector(`#sdk-${kind}`).onchange = async event => {
        const generation = view.generation, value = event.target.value;
        if (!value && !(kind === 'effort' && event.target.selectedOptions[0]?.disabled === false)) return;
        try { await settings(view, {[kind]: value}); }
        catch (error) { if (view.current(generation)) view.setNotice(`设置失败：${error.message}`, 'blocked', true); }
      };
    }
  },

  closeToolbar(view) {
    view.toolbarObserver?.disconnect(); view.toolbarObserver = null;
    view.toolbarBindings?.abort(); view.toolbarBindings = null;
    view.toolbarWidth = null; view.toolbarLayout = null;
    view.closeToolbarMenus();
  },

  decorateUserBubble(view, node, items) {
    if (!items.length || node.querySelector('.sdk-bubble-toggle')) return;
    node.classList.add('sdk-user-actions');
    const toggle = el('button', 'sdk-bubble-toggle', '⋯');
    toggle.type = 'button';
    toggle.setAttribute('aria-label', '消息操作');
    toggle.setAttribute('aria-expanded', 'false');
    const menu = el('div', 'new-model-menu sdk-bubble-menu');
    menu.setAttribute('popover', 'auto');
    menu.setAttribute('aria-label', '消息操作');
    menu.addEventListener('beforetoggle', event => toggle.setAttribute('aria-expanded', String(event.newState === 'open')));
    for (const [label, handler] of items) {
      const button = el('button', 'new-model-option', label);
      button.type = 'button';
      button.onclick = () => { menu.hidePopover(); void handler(); };
      menu.append(button);
    }
    toggle.onclick = () => {
      if (menu.matches(':popover-open')) { menu.hidePopover(); return; }
      view.closeToolbarMenus();
      menu.showPopover();
      view.placeMenu(menu, toggle, 180);
    };
    node.append(toggle, menu);
  },

  paintHistory(view, messages, {tailComplete = true, decorate} = {}) {
    const folds = new Map([...view.history.querySelectorAll(':scope > .turn-process')]
      .map(node => [node.dataset.turnId, node.classList.contains('folded')]));
    view.history.replaceChildren();
    const partial = view.historyData.partial;
    const split = partial ? Math.min(partial.head, messages.length) : 0;
    const visible = message => message.event_kind !== 'compact';
    const plan = partial
      ? [...planTurns(messages.slice(0, split).filter(visible), {tailComplete: false, foldTail: true}),
         {gap: {uid: view.uid, agent: view.agent, omitted: partial.omitted}},
         ...planTurns(messages.slice(split).filter(visible), {tailComplete, openTail: !tailComplete})]
      : planTurns(messages.filter(visible), {tailComplete, openTail: !tailComplete});
    const built = buildPlan(view.history, plan, null);
    if (decorate) built.forEach((node, index) => decorate(node, plan[index].m));
    for (const node of view.history.querySelectorAll(':scope > .turn-process')) {
      if (folds.has(node.dataset.turnId))
        folds.get(node.dataset.turnId) ? node._fold?.() : node._open?.();
    }
    refreshMessageTimeDividers(view.history);
  },

  methods: {
    current(generation) { return generation === this.generation && !!this.pane; },
    fitInput() {
      if (!this.input) return;
      this.input.classList.toggle('shell-mode', /^\s*(?:!|\/shell(?:\s|$))/.test(this.input.value));
      ConversationUI.resizeInput(this.input);
    },
    placeMenu(menu, button, width) { ConversationUI.placePopover(menu, button, width); },
    closeToolbarMenus() { ConversationUI.closePopovers(this.pane); },
    updateStatusDisclosure() {
      if (this.notice) ConversationUI.updateStatusDisclosure(this.notice, this.pane.querySelector('#sdk-status-menu'));
    },
    setNotice(text, kind = '', sticky = false) {
      if (!this.notice) return;
      if (this.readOnly && kind === 'connecting') { text = '未运行'; kind = 'idle'; }
      this.notice.querySelector('[role=status]').textContent = ConversationUI.statusSummary(text, kind);
      this.notice.title = text;
      this.notice.hidden = !text;
      this.notice.className = 'sdk-status' + (kind ? ` ${kind}` : '');
      this.notice.dataset.state = kind || 'neutral';
      this.notice.dataset.sticky = sticky ? '1' : '';
      this.pane.querySelector('#sdk-status-menu').textContent = text;
      this.updateStatusDisclosure();
      this.layoutToolbar();
      sdkSyncSettings(this);
    },
    mountToolbar(generation) {
      this.toolbarBindings = new AbortController();
      for (const [trigger, id] of [['#sdk-more', '#sdk-more-menu'], ['#sdk-usage-toggle', '#sdk-usage-menu'], ['.sdk-status', '#sdk-status-menu']]) {
        const button = this.pane.querySelector(trigger), menu = this.pane.querySelector(id);
        if (button && menu?.hasAttribute('popover')) ConversationUI.bindPopover(button, menu, this.pane);
      }
      window.addEventListener('resize', () => this.closeToolbarMenus(), {signal: this.toolbarBindings.signal});
      this.toolbarObserver = new ResizeObserver(() => {
        if (this.current(generation)) this.layoutToolbar(this.pane.querySelector('.sdk-status-row').clientWidth);
      });
      this.toolbarObserver.observe(this.pane.querySelector('.sdk-status-row'));
      this.toolbarObserver.observe(this.notice);
      this.layoutToolbar();
    },
    pending(uid) {
      return typeof T === 'undefined' || S.sessions.some(row => row.uid === uid) ? null
        : T.sdkSessions?.find(row => row.uid === uid) || null;
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
        if (this.connected && (data.observedActivity?.state !== observed?.state
            || data.observedActivity?.reason !== observed?.reason)) this.paintStatus();
        data.observedActivity = observed;
        return;
      }
      this.historyEntry = entry;
      this.historyData = {messages: entry.msgs, partial: entry.partial,
        activity: entry.activity, observedActivity: observed, end: entry.end, length: entry.msgs.length};
      this.paintHistory();
      if(this.connected)this.paintStatus();
    },

    scheduleHistory() {
      clearTimeout(this.historyTimer);
      const generation = this.generation;
      this.historyTimer = setTimeout(() => this.refreshHistory(generation, true), 600);
    },

    setConnected(connected) {
      this.connected = connected;
      if (this.pane) sdkSyncSettings(this);
      if (!this.uid || this.agent) return;
      paintItemStatus(document.querySelector(`.item[data-uid="${CSS.escape(this.uid)}"]`));
      if (S.sel === this.uid) paintHeaderTurn();
    },

    nearBottom() {
      const box = this.box;
      return !box || box.scrollHeight - box.scrollTop - box.clientHeight < 80;
    },

    scrollBottom() {
      if (this.box) this.box.scrollTop = this.box.scrollHeight;
    },

  },
  layoutFooter(view, controls) {
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
  },
};
