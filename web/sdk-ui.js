'use strict';

// Source-neutral SDK view contract. Providers own snapshots, status semantics,
// settings and protocol actions; history uses the ordinary conversation cache.
const SdkUI = {
  reconnectEvents(view) {
    if (!view.pane || view.readOnly || !view.es
        || view.uid !== S.sel || view.agent !== S.agent) return;
    clearTimeout(view.reconnectTimer);
    view.es.close();
    view.es = null;
    view.setConnected(false);
    view.setNotice('重连中', 'connecting');
    // Only replace the browser subscription; do not add an SDK open request,
    // clear live output, or replay an input whose outcome is unknown.
    view.connectEvents(view.generation);
  },
  titleRenameView(heading) {
    const view = SdkView.active;
    return heading?.dataset.uid === S.sel && SessionContentView.kind === 'sdk'
      && view.uid === S.sel && !view.agent && view.connected && !view.readOnly
      && (view === CodexSdkView || view.capabilities?.rename === true) ? view : null;
  },
  syncTitleRename(heading = document.querySelector('#detail > .dhead')) {
    let title = heading?.querySelector('#a-session-title');
    if (!title) return;
    const editable = !!this.titleRenameView(heading);
    const switcher = heading.querySelector('#a-view-switch');
    if (editable !== (title.tagName === 'BUTTON')) {
      const next = document.createElement(editable ? 'button' : 'span');
      next.id = title.id;
      next.textContent = title.textContent;
      if (editable) {
        next.type = 'button';
        next.className = 'session-title-edit';
        next.setAttribute('aria-label', '重命名会话');
        next.title = '重命名会话';
        next.onclick = () => this.renameTitle(heading);
      }
      title.replaceWith(next);
      title = next;
    }
    // Keep the agent selector separate from the editable title: no nested buttons.
    if (switcher && editable && switcher.contains(title)) switcher.before(title);
    if (switcher && !editable && !switcher.contains(title)) switcher.prepend(title);
    if (editable) title.disabled = !!this.titleRenamePending;
  },
  async renameTitle(heading) {
    const view = this.titleRenameView(heading);
    if (!view || this.titleRenamePending) return;
    this.titleRenamePending = true;
    this.syncTitleRename(heading);
    try {
      if (view === CodexSdkView) await CodexSdkActions.command(view, '/rename', {preserveDraft: true});
      else await NativeSdkControls.rename(view, null, '');
    } finally {
      this.titleRenamePending = false;
      this.syncTitleRename();
    }
  },
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
    // Same notion of an active turn as the send button (paintSend).
    view.box._followLive = () => !!(view.uid?.startsWith('codex:') ? view.turn : view.running);
    watchBottom(view.box);
    view.history = pane.querySelector('.sdk-history');
    if (historyReady) view.history.innerHTML = '<div class="spin">正在读取会话…</div>';
    view.live = pane.querySelector('.sdk-live');
    view.toolGroupNodes = new Map();
    view.notice = pane.querySelector('.sdk-status');
    view.input = pane.querySelector('#sdk-input');
    pane.querySelector('#sdk-composer').hidden = false;
    view.input.disabled = true;
    pane.querySelector('#sdk-send').disabled = true;
    view.input.oninput = () => { view.fitInput(); CodexSdkCompose.input(view); };
    view.input.onblur = event => PathCompletion.close(event.currentTarget);
    view.input.onkeydown = event => {
      if (PathCompletion.key(view.input, event, () => view.uid)) return;
      if (this.canSteer(view) && event.key === 'Enter' && event.ctrlKey && !event.shiftKey && !event.isComposing) {
        event.preventDefault(); void view.send('steer'); return;
      }
      ConversationUI.submitOnEnter(event, () => view.send());
    };
    pane.querySelector('#sdk-send').onclick = () => view.send();
    pane.querySelector('#sdk-stop').onclick = () => view.interrupt();
    // Match the conversation editor: clicking SEND must not leave focus on a
    // disabled button while the request is pending.
    pane.querySelector('#sdk-send').onmousedown = event => event.preventDefault();
    this.bindSteerSend(view);
    for (const kind of ['model', 'effort']) {
      pane.querySelector(`#sdk-${kind}`).onchange = async event => {
        const generation = view.generation, value = event.target.value;
        if (!value && !(kind === 'effort' && event.target.selectedOptions[0]?.disabled === false)) return;
        try { await settings(view, {[kind]: value}); }
        catch (error) { if (view.current(generation)) view.setNotice(`设置失败：${error.message}`, 'blocked', true); }
      };
    }
  },

  canSteer(view) {
    const source=view.uid.split(':')[0];
    return ['codex','kiro'].includes(source) || ['droid','agy'].includes(source) && !!view.capabilities?.steer;
  },

  paintSend(view) {
    const button = view.pane?.querySelector('#sdk-send');
    if (!button || !view.steerSend) return;
    const active = view.uid.startsWith('codex:') ? !!view.turn : view.running;
    const supported = this.canSteer(view);
    const steer = supported && active && (view.steerSend.ctrl || view.steerSend.held);
    const label = steer ? '引导' : active ? '排队' : '发送';
    const title = steer ? '引导当前任务' : supported ? '点击发送／排队；按住 Ctrl 点击或手机长按可引导当前任务' : active ? '当前任务结束后发送；尚未投递的排队消息可撤销' : '开始新回合';
    // Keyboard events also arrive for ordinary typing. Replacing unchanged text
    // invalidates ancestor :has() rules and needlessly restyles long histories.
    if (button.textContent !== label) button.textContent = label;
    if (button.title !== title) button.title = title;
  },

  bindSteerSend(view) {
    if (!['codex', 'kiro', 'droid', 'agy'].includes(view.uid.split(':')[0])) return;
    const button = view.pane.querySelector('#sdk-send'), generation = view.generation;
    const controller = new AbortController(), {signal} = controller;
    const state = view.steerSend = {ctrl:false, held:false, controller};
    let timer, pointer, consumed = false;
    const current = () => view.current(generation) && view.steerSend === state;
    const cancel = (suppress = false) => {
      clearTimeout(timer); timer = null;
      if (suppress && pointer) consumed = true;
      pointer = null; state.held = false;
      if (current()) this.paintSend(view);
    };
    const reset = () => { state.ctrl = false; cancel(true); };
    signal.addEventListener('abort', reset, {once:true});
    button.classList.add('sdk-steer-send');
    for (const type of ['keydown', 'keyup']) document.addEventListener(type, event => {
      if (!current()) return;
      if (type === 'keydown' && !pointer) consumed = false;
      state.ctrl = event.ctrlKey; this.paintSend(view);
    }, {signal});
    window.addEventListener('blur', reset, {signal});
    document.addEventListener('visibilitychange', () => { if (document.hidden) reset(); }, {signal});
    button.addEventListener('pointerdown', event => {
      consumed = false; cancel();
      if (event.pointerType !== 'touch' || !event.isPrimary || button.disabled || !this.canSteer(view)) return;
      pointer = {id:event.pointerId, x:event.clientX, y:event.clientY};
      timer = setTimeout(() => {
        if (!pointer || !current() || button.disabled || !this.canSteer(view)) return;
        consumed = true; state.held = true; this.paintSend(view);
        void view.send('steer');
      }, 500);
    }, {signal});
    button.addEventListener('pointermove', event => {
      if (pointer && (Math.abs(event.clientX-pointer.x)>10 || Math.abs(event.clientY-pointer.y)>10)) cancel(true);
    }, {signal});
    button.addEventListener('pointerleave', () => cancel(true), {signal});
    button.addEventListener('pointercancel', () => cancel(true), {signal});
    window.addEventListener('pointerup', event => { if (pointer?.id === event.pointerId) cancel(); }, {signal});
    button.addEventListener('contextmenu', event => { if (pointer || consumed || event.pointerType === 'touch') event.preventDefault(); }, {signal});
    button.onclick = event => {
      if (consumed) { consumed = false; event.preventDefault(); return; }
      if (current()) void view.send(this.canSteer(view) && (event.ctrlKey || state.ctrl) ? 'steer' : 'turn');
    };
    this.paintSend(view);
  },

  closeToolbar(view) {
    view.steerSend?.controller.abort(); view.steerSend = null;
    view.toolbarObserver?.disconnect(); view.toolbarObserver = null;
    view.toolbarBindings?.abort(); view.toolbarBindings = null;
    view.toolbarWidth = null;
    view.closeToolbarMenus();
  },

  messageTime(view, message) {
    const scope = JSON.stringify([view.uid, view.agent]);
    if (view.messageTimeScope !== scope) {
      view.messageTimeScope = scope; view.messageTimes = new Map(); view.messageTimeHistory = null;
    }
    const keys = row => {
      const ids = [row.native_item_id, row.native_id, row.message_id, row.id].filter(Boolean);
      if (row.role === 'user' && !String(view.uid).startsWith('codex:'))
        ids.push(NativeSdkControls.messageIdentity(view, row));
      return ids.filter(Boolean).map(id => `${row.role}:${id}`);
    };
    const validTime = row => {
      const value = row.ts || row.timestamp;
      return value && Number.isFinite(new Date(value).getTime()) ? value : null;
    };
    const remember = row => {
      const time = validTime(row);
      if (time) for (const key of keys(row)) view.messageTimes.set(key, time);
    };
    const history = view.historyData?.messages || [];
    const replay = JSON.stringify([view.turn?.id, view.messageTimeReplayStart]);
    if (view.messageTimeHistory !== history || view.messageTimeReplay !== replay) {
      view.messageTimeHistory = history;
      view.messageTimeReplay = replay;
      view.messageTextTimes = new Map();
      for (const [index, row] of history.entries()) {
        remember(row);
        const time = validTime(row);
        // The existing replay join also establishes a turn boundary for older
        // Codex records that omit both item and turn IDs. It does not authorize actions.
        const turn = row.turn_id || (view.messageTimeReplayStart != null
          && index >= view.messageTimeReplayStart ? view.turn?.id : null);
        if (time && turn && row.text) {
          const key = JSON.stringify([turn, row.role, row.text]);
          view.messageTextTimes.set(key, view.messageTextTimes.has(key) ? null : time);
        }
      }
    }
    let time = validTime(message) || keys(message).map(key => view.messageTimes.get(key)).find(Boolean);
    // Older Codex rollouts omit item IDs. Only reuse an unambiguous matching
    // message inside the same native turn; equal prose in other turns is unrelated.
    if (!time && message.turn_id && message.text) {
      time = view.messageTextTimes.get(JSON.stringify([message.turn_id, message.role, message.text]));
    }
    const result = time ? {...message, ts: time} : message;
    remember(result);
    return result;
  },

  refreshMessageTimes(view) {
    for (const node of view.nodes?.values() || []) {
      if (node._messageData) node._updateMessageHeader?.(this.messageTime(view, node._messageData));
    }
  },

  decorateUserBubble(view, node, items) {
    if (!items.length || node.querySelector('.sdk-bubble-menu')) return;
    node.classList.add('sdk-user-actions');
    const menu = el('div', 'sdk-bubble-menu');
    menu.setAttribute('role', 'group');
    menu.setAttribute('aria-label', '消息操作');
    for (const [label, handler] of items) {
      const button = el('button', 'message-action');
      button.innerHTML = uiIcon(label.includes('回退') ? 'undo' : 'fork');
      button.type = 'button';
      button.title = label; button.setAttribute('aria-label', label);
      button.onclick = async () => {
        button.disabled = true;
        try { await handler(); } finally { button.disabled = false; }
      };
      menu.append(button);
    }
    node.querySelector('.message-actions')?.append(menu);
  },

  ungroupTools(view) {
    const identity = `${view.uid}:${view.agent || ''}`;
    if (view.toolGroupIdentity !== identity) {
      view.toolGroupIdentity = identity; view.toolGroupChoices = new Map();
      view.toolGroupNodes = new Map();
    }
    // A snapshot can reconcile history after already unpacking the live tools.
    // Keep the saved groups through that nested pass until grouping commits.
    view.toolGroupNodes ||= new Map();
    for (const group of view.live?.querySelectorAll('.sdk-tool-group') || []) {
      view.toolGroupNodes.set(group.dataset.toolGroupKey, group);
      view.toolGroupChoices.set(group.dataset.toolGroupKey, !!group._userOpened);
      group.replaceWith(...group.querySelectorAll(':scope > [data-sdk-item]'));
    }
  },

  groupTools(view) {
    const retained = new Map([...view.live.querySelectorAll('.sdk-tool-group')]
      .map(group => [group.dataset.toolGroupKey, group]));
    for (const parent of [view.live, ...view.live.querySelectorAll('.turn-process-body')]) {
      let run = [];
      const flush = () => {
        if (run.length >= GROUP_MIN) {
          const items = run.map(node => node._sdkToolMessage);
          const key = run[0].dataset.sdkItem;
          const anchor = run.at(-1).nextSibling;
          const running = view.running ?? (view.turn?.status === 'inProgress');
          const previous = view.toolGroupNodes?.get(key);
          const group = previous || groupNode(items, !!running, run);
          if (previous) {
            const currentRun = new Set(run);
            for (const stale of [...previous.querySelectorAll(':scope > [data-sdk-item]')]) {
              if (!currentRun.has(stale)) stale.remove();
            }
            group._syncExternalTools(items, run);
          }
          group.classList.add('sdk-tool-group');
          group.dataset.toolGroupKey = key;
          parent.insertBefore(group, anchor);
          retained.set(key, group);
          if (!previous && view.toolGroupChoices?.get(key)) { group._userOpened = true; group._open(); }
        }
        run = [];
      };
      for (const node of [...parent.children]) {
        if (node._sdkToolMessage && isGroupableTool(node._sdkToolMessage)) run.push(node);
        else flush();
      }
      flush();
    }
    view.toolGroupNodes = retained;
    ConversationUI.refreshToolTails(view.box);
    markMatches(view.live);
    updateMatchNav();
  },

  foldLiveTurns(view, messages, {tailComplete}) {
    // Native snapshots can replay the whole session. Apply the same completed
    // turn boundaries as history, but retain the streaming message/tool nodes.
    const plan = planTurns(messages.map(message => ({...message,
      turn_id: message.turn_id || String(message.id)})), {tailComplete, openTail: !tailComplete});
    const previous = view.liveTurns, groups = new Map(), membership = new Map();
    const place = (parent, nodes) => {
      let before = parent.firstElementChild;
      for (const node of nodes) {
        if (node === before) before = before.nextElementSibling;
        else parent.insertBefore(node, before);
      }
    };
    for (const {turn, open} of plan) {
      if (!turn) continue;
      const ids = turn.items.map(item => String(item.id));
      const key = `${turn.key}:${ids[0]}`;
      const shape = JSON.stringify([ids, turnProcessSummary(turn.items),
        turn.hasConclusion, turn.inferred, turn.interrupted]);
      let node = previous.get(key);
      if (!node || node._sdkTurnShape !== shape) {
        const expanded = node ? !node.classList.contains('folded') : open;
        // These messages are already materialized by the protocol adapter.
        // An empty plan prevents the disclosure from creating duplicate nodes.
        node = turnProcessNode({...turn, plan: []}, expanded);
        node._sdkTurnShape = shape;
      }
      node._turnItems = turn.items;
      place(node.querySelector('.turn-process-body'), ids.map(id => view.nodes.get(id)).filter(Boolean));
      groups.set(key, node);
      for (const id of ids) membership.set(id, node);
    }
    const ordered = [], seen = new Set();
    for (const message of messages) {
      const id = String(message.id), node = membership.get(id) || view.nodes.get(id);
      if (node && !seen.has(node)) { ordered.push(node); seen.add(node); }
    }
    place(view.live, ordered);
    for (const [key, node] of previous) if (groups.get(key) !== node) node.remove();
    view.liveTurns = groups;
  },

  paintHistory(view, messages, {tailComplete = true, decorate, actions = ''} = {}) {
    S.autoOpen = 0; S.markCapped = false;
    const folds = new Map([...view.history.querySelectorAll(':scope > .turn-process')]
      .map(node => [node.dataset.turnId, node.classList.contains('folded')]));
    const partial = view.historyData.partial;
    const split = partial ? Math.min(partial.head, messages.length) : 0;
    const visible = message => message.event_kind !== 'compact';
    const plan = partial
      ? [...planTurns(messages.slice(0, split).filter(visible), {tailComplete: false, foldTail: true}),
         {gap: {uid: view.uid, agent: view.agent, omitted: partial.omitted}},
         ...planTurns(messages.slice(split).filter(visible), {tailComplete, openTail: !tailComplete})]
      : planTurns(messages.filter(visible), {tailComplete, openTail: !tailComplete});
    // History appends and native replay joins usually leave the older turns
    // untouched. Reuse their nodes, including text selections and disclosures.
    const equalContent = (a, b) => {
      if (a === b) return true;
      if (!a || !b || typeof a !== 'object' || typeof b !== 'object'
          || Array.isArray(a) !== Array.isArray(b)) return false;
      const keys = Object.keys(a);
      return keys.length === Object.keys(b).length
        && keys.every(key => Object.hasOwn(b, key) && equalContent(a[key], b[key]));
    };
    const equalItems = (a, b) => a?.length === b?.length && a.every((item, i) => equalContent(item, b[i]));
    const key = p => p.m || p.g?.[0] || p.turn?.items[0] || 'gap';
    const unchanged = (a, b) => a.m ? equalContent(a.m, b.m)
      : a.g ? !!b.g && equalItems(a.g, b.g) && a.open === b.open
      : a.turn ? !!b.turn && a.turn.key === b.turn.key && equalItems(a.turn.items, b.turn.items)
        && a.turn.hasConclusion === b.turn.hasConclusion && a.turn.inferred === b.turn.inferred
        && a.turn.interrupted === b.turn.interrupted
      : !!b.gap && a.gap.omitted === b.gap.omitted;
    const previous = view.history._sdkPlan;
    const old = new Map(previous?.actions === actions ? previous.entries.map(entry => [entry.key, entry]) : []);
    const entries = [], kept = new Set();
    let before = view.history.firstChild;
    for (const item of plan) {
      // A bounded reset can deserialize the same content into new objects.
      // Compare that slot's complete message data before reusing its UI.
      const prior = old.get(key(item)) || (previous?.actions === actions ? previous.entries[entries.length] : null);
      let node = prior?.node;
      if (!node || kept.has(node) || node.parentNode !== view.history || !unchanged(prior.plan, item)) {
        [node] = buildPlan(view.history, [item], before);
        if (decorate) decorate(node, item.m);
        if (item.turn && folds.has(node.dataset.turnId))
          folds.get(node.dataset.turnId) ? node._fold?.() : node._open?.();
      } else if (node !== before) view.history.insertBefore(node, before);
      before = node.nextSibling;
      // Activity annotates interruption fields in place; media continuation
      // replaces media fields. Retain the displayed fields, not that mutable row.
      const saved = item.m ? {...item, m: {...item.m}}
        : item.g ? {...item, g: item.g.map(message => ({...message}))}
        : item.turn ? {...item, turn: {...item.turn, items: item.turn.items.map(message => ({...message}))}}
        : item;
      entries.push({key: key(item), plan: saved, node}); kept.add(node);
    }
    for (const node of [...view.history.childNodes]) if (!kept.has(node)) node.remove();
    view.history._sdkPlan = {entries, actions};
    markMatches(view.history);
    updateMatchNav();
    // History is only part of a snapshot: live tools may still be unpacked.
    // Measure/scroll after the complete history + live reconciliation.
    const box = view.box;
    queueMicrotask(() => { if (box.isConnected) stickBottom(box); });
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
      this.baseNotice = [text, kind, sticky];
      if (this.readOnly && kind === 'connecting') { text = '未运行'; kind = 'idle'; }
      this.notice.querySelector('[role=status]').textContent = ConversationUI.statusSummary(text, kind);
      this.notice.title = text;
      this.notice.hidden = !text;
      this.notice.className = 'sdk-status' + (kind ? ` ${kind}` : '');
      this.notice.dataset.state = kind || 'neutral';
      this.notice.dataset.sticky = sticky ? '1' : '';
      this.pane.querySelector('#sdk-status-menu').textContent = text;
      paintComposerRunControl(this.notice, this.uid, this.agent, 'sdk', this);
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
      this.setNotice(this.readOnly ? '未运行' : '连接中', this.readOnly ? 'idle' : 'connecting');
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
          && data.partial === entry.partial
          && data.activity?.state === entry.activity?.state
          && data.activity?.turn_id === entry.activity?.turn_id
          && data.activity?.reason === entry.activity?.reason) {
        // A poll can deliver a fresh activity object without changing history.
        // Keep the text nodes (and an ongoing native drag selection) intact.
        const activityChanged = data.activity !== entry.activity;
        data.activity = entry.activity;
        // CLI-only updates can close a native turn whose ending was never
        // written. They must refresh status even when history is unchanged.
        const observedChanged = data.observedActivity?.state !== observed?.state
          || data.observedActivity?.reason !== observed?.reason;
        data.observedActivity = observed;
        if (observedChanged && !this.connected) this.paintHistory();
        if (this.connected && (activityChanged || observedChanged)) this.paintStatus();
        return;
      }
      this.historyEntry = entry;
      this.historyData = {messages: entry.msgs, partial: entry.partial,
        activity: entry.activity, observedActivity: observed, end: entry.end, length: entry.msgs.length};
      measureBrowserWork('sdk.history', () => this.paintHistory(),
        {source: this.uid.split(':')[0], messages: entry.msgs.length});
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
      return !box || messageScrollState(box).stick;
    },

    scrollBottom() {
      if (this.box) stickBottom(this.box, true);
    },

  },
  appendQuestionFields(body, questions, {namePrefix, codex = false, formatAnswer}) {
    return questions.map((field, index) => {
      const group = document.createElement('fieldset');
      group.className = 'sdk-question';
      const title = codex ? field.question : (field.question || field.title || field.key || '回答');
      const legend = document.createElement('legend');
      legend.textContent = [field.header, title].filter(Boolean).join(' · ');
      group.append(legend);
      const multiple = !codex && Boolean(field.multiSelect || field.multiple || field.type === 'array');
      const name = codex ? `${namePrefix}-${field.id}` : `${namePrefix}-${index}`;
      for (const option of field.options || []) {
        const label = document.createElement('label'), input = document.createElement('input');
        const value = codex ? option.label : (typeof option === 'string' ? option : option.value ?? option.label ?? option.id);
        input.type = multiple ? 'checkbox' : 'radio';
        input.name = name;
        input.value = String(value);
        label.append(input, ` ${codex ? option.label : (typeof option === 'string' ? option : option.label || option.title || String(value))}`);
        if (option.description) label.title = option.description;
        group.append(label);
      }
      let other = null;
      if (!codex || field.isOther || !(field.options || []).length) {
        other = document.createElement('input');
        other.type = codex && field.isSecret ? 'password' : 'text';
        other.placeholder = (field.options || []).length ? '其他答案' : '输入答案';
        if (!codex) other.setAttribute('aria-label', title);
        group.append(other);
      }
      body.append(group);
      return () => {
        const picked = [...group.querySelectorAll('input:checked')].map(input => input.value);
        const typed = other?.value.trim() || '';
        return formatAnswer(field, index, {picked, typed, multiple});
      };
    });
  },
  layoutFooter(view, controls) {
    const row = view.pane.querySelector('.sdk-status-row'), gauges = row.querySelector('.sdk-gauges');
    const pickers = row.querySelector('.sdk-pickers'), menu = view.pane.querySelector('#sdk-more-menu');
    const trigger = view.pane.querySelector('#sdk-more');
    const list = controls.filter(Boolean);
    const rowWidth = row.clientWidth;
    const controlsMetaKey = JSON.stringify([rowWidth, row.querySelector('.composer-run-action')?.hidden,
      [...menu.querySelectorAll('.sdk-control-note, .sdk-reset')].map(n => [n.hidden, n.textContent]),
      list.map(n => [n.hidden, n.textContent, n.querySelector('select')?.value])]);
    const signature = JSON.stringify([view.notice.textContent, gauges.textContent,
      row.querySelector(':scope > .sdk-credits')?.textContent, controlsMetaKey]);
    const hadLayout = Boolean(row.dataset.footerLayout);
    if (row.dataset.footerLayout === signature) return;
    row.dataset.footerLayout = signature;
    if (gauges.tabIndex !== 0) gauges.tabIndex = 0;
    if (row.dataset.footerWidth && Number(row.dataset.footerWidth) !== rowWidth) view.closeToolbarMenus();
    row.dataset.footerWidth = rowWidth;
    const actionsBox = view.pane.querySelector('#sdk-actions');
    const overflow = control => control.classList.contains('sdk-action') ? actionsBox : menu;
    const fits = () => row.scrollWidth <= row.clientWidth + 1 && gauges.scrollWidth <= gauges.clientWidth + 1;
    const rowSlack = () => {
      const style = getComputedStyle(row);
      const pad = (parseFloat(style.paddingLeft) || 0) + (parseFloat(style.paddingRight) || 0);
      const gap = parseFloat(style.columnGap) || 0;
      let used = pad, count = 0;
      for (const child of row.children) {
        if (getComputedStyle(child).display === 'none') continue;
        used += child.getBoundingClientRect().width;
        count++;
      }
      if (count > 1) used += gap * (count - 1);
      return row.clientWidth - used;
    };
    const prev = row._footerState;
    if (hadLayout && prev && prev.controlsMetaKey === controlsMetaKey
        && prev.nodes.length === list.length && prev.nodes.every((n, i) => n === list[i])
        && list.every((n, i) => n.parentElement === (i < prev.keep ? pickers : overflow(n)))) {
      if (fits()) {
        const slack = (prev.keep === list.length && trigger.hidden) || prev.keep < list.length ? rowSlack() : 0;
        if (prev.keep === list.length) {
          if (!trigger.hidden || slack >= (prev.triggerReserve || 38)) return;
        } else if (slack + 2 < prev.nextOverflowWidth) {
          return;
        }
      }
    }
    // Start with status, usage and ⋯; optional controls never displace usage.
    if (trigger.hidden) trigger.hidden = false;
    for (const control of list) {
      if (!control.matches('.new-effort')) continue;
      const select = control.querySelector('select');
      let label = control.querySelector(':scope > .visually-hidden, :scope > .sdk-picker-label');
      if (!label && select) {
        label = el('span', '', select.getAttribute('aria-label') || control.title);
        control.prepend(label);
      }
      if (label) {
        label.classList.add('visually-hidden');
        label.classList.remove('sdk-picker-label');
      }
    }
    const anchor = pickers.querySelector('.sdk-fixed-action') || trigger;
    let cursor = anchor;
    for (let i = list.length - 1; i >= 0; i--) {
      const control = list[i];
      if (control.parentElement !== pickers || control.nextElementSibling !== cursor) {
        pickers.insertBefore(control, cursor);
      }
      cursor = control;
    }
    let keep = list.length;
    const widths = new Array(list.length).fill(0);
    let pickersGap = 0;
    const rCw = row.clientWidth, rSw = row.scrollWidth, gCw = gauges.clientWidth, gSw = gauges.scrollWidth;
    const triggerReserve = Math.ceil((trigger.getBoundingClientRect().width || 24) + 14);
    if (rSw > rCw + 1 || gSw > gCw + 1) {
      pickersGap = parseFloat(getComputedStyle(pickers).columnGap) || 0;
      for (let i = 0; i < list.length; i++) {
        widths[i] = list[i].hidden ? 0 : list[i].getBoundingClientRect().width;
      }
      const excess = Math.max(0, rSw - rCw) + Math.max(0, gSw - gCw);
      let saved = 0;
      while (keep > 0 && saved < excess - 1) {
        keep--;
        if (widths[keep] > 0) saved += widths[keep] + pickersGap;
      }
      for (let i = keep; i < list.length; i++) overflow(list[i]).append(list[i]);
      while (keep > 0 && !fits()) {
        keep--;
        const control = list[keep], target = overflow(control);
        const nextSibling = list.slice(keep + 1).find(c => overflow(c) === target) || null;
        target.insertBefore(control, nextSibling);
      }
      while (keep < list.length) {
        const candidate = list[keep];
        if (!candidate.hidden && saved - (widths[keep] + pickersGap) < excess - 2) break;
        pickers.insertBefore(candidate, anchor);
        if (!fits()) {
          const target = overflow(candidate);
          const nextSibling = list.slice(keep + 1).find(c => overflow(c) === target) || null;
          target.insertBefore(candidate, nextSibling);
          break;
        }
        if (widths[keep] > 0) saved -= widths[keep] + pickersGap;
        keep++;
      }
    }
    const overflowPickers = list.filter(control => control.matches('.new-effort') && control.parentElement === menu);
    if (overflowPickers.length) menu.prepend(...overflowPickers);
    for (const control of list) {
      if (!control.matches('.new-effort')) continue;
      const inMenu = control.parentElement === menu;
      const label = control.querySelector(':scope > .visually-hidden, :scope > .sdk-picker-label');
      label?.classList.toggle('visually-hidden', !inMenu);
      label?.classList.toggle('sdk-picker-label', inMenu);
    }
    // Empty wrappers and hidden reset cards are not menu contents. The same
    // contents decide both disclosure visibility and boundaries between groups.
    const hasContent = node => !node.hidden && (node.matches('button, input, select, textarea')
      || [...node.childNodes].some(child => child.nodeType === Node.TEXT_NODE
        ? !!child.textContent.trim() : child.nodeType === Node.ELEMENT_NODE && hasContent(child)));
    const hasChildContent = parent => [...parent.childNodes].some(child =>
      child.nodeType === Node.TEXT_NODE ? !!child.textContent.trim()
        : child.nodeType === Node.ELEMENT_NODE && hasContent(child));
    for (const group of menu.querySelectorAll(':scope > #sdk-actions, :scope > #sdk-permissions')) {
      const nextHidden = !hasChildContent(group);
      if (group.hidden !== nextHidden) group.hidden = nextHidden;
    }
    const nextTriggerHidden = !hasChildContent(menu);
    if (trigger.hidden !== nextTriggerHidden) trigger.hidden = nextTriggerHidden;
    if (trigger.hidden && menu.matches(':popover-open')) {
      menu.hidePopover();
      view.input?.focus();
    }
    let nextOverflowWidth = Infinity;
    for (let i = keep; i < list.length; i++) {
      if (!list[i].hidden) { nextOverflowWidth = (widths[i] || 0) + pickersGap; break; }
    }
    row._footerState = {controlsMetaKey, nodes: list, keep, nextOverflowWidth, triggerReserve};
  },
};
