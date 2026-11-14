'use strict';

// Presentation shared by SDK and CLI-backed conversation views. Callers own
// transport, draft identity and delivery evidence; these helpers never send.
const ConversationUI = {
  messageHeader(node, message) {
    const header = el('div', 'message-header');
    const time = el('time', 'message-time');
    const actions = el('div', 'message-actions');
    actions.setAttribute('role', 'group');
    actions.setAttribute('aria-label', '消息操作');
    const copy = el('button', 'message-action');
    copy.innerHTML = uiIcon('copy');
    copy.type = 'button'; copy.title = '复制原文（保留链接地址）';
    copy.setAttribute('aria-label', '复制消息原文');
    copy.onclick = async () => {
      copy.disabled = true;
      try {
        await copyFileText(String(message.text || ''));
        copy.innerHTML = uiIcon('check'); copy.title = '已复制原文'; copy.dataset.copyState = 'copied';
      }
      catch (error) { copy.innerHTML = uiIcon('info'); copy.title = `复制失败：${error.message}`; copy.dataset.copyState = 'failed'; }
      finally { copy.disabled = false; }
    };
    actions.append(copy); header.append(time, actions); node.prepend(header);
    let knownTime = '', knownSource = '';
    const update = next => {
      message = next;
      node._messageData = next;
      const stamp = message.ts || message.timestamp || '';
      if (Number.isFinite(new Date(stamp).getTime())) { knownTime = stamp; knownSource = '原始时间'; }
      else if (knownSource !== '原始时间' && Number.isFinite(new Date(message.received_at || '').getTime())) {
        knownTime = message.received_at; knownSource = '后台接收时间';
      }
      const date = new Date(knownTime);
      const valid = Number.isFinite(date.getTime());
      const value = valid ? date.toLocaleString('sv-SE', {hour12: false}) : '';
      time.hidden = !valid;
      if (time.textContent !== value) time.textContent = value;
      if (valid) { time.dateTime = date.toISOString(); time.title = `${knownSource} · 本地时间 · ${knownTime}`; }
      else { time.removeAttribute('datetime'); time.removeAttribute('title'); }
    };
    update(message);
    node._updateMessageHeader = update;
    return update;
  },

  resizeInput(input, {minHeight = 0, maxHeight = 240} = {}) {
    if (!input) return;
    input.style.height = 'auto';
    const height = input.scrollHeight;
    input.style.height = Math.min(maxHeight, Math.max(minHeight, height)) + 'px';
    input.style.overflowY = height > maxHeight ? 'auto' : 'hidden';
  },

  submitOnEnter(event, submit) {
    if (event.key !== 'Enter' || event.shiftKey || event.isComposing || MOBILE.matches) return false;
    event.preventDefault();
    void submit();
    return true;
  },

  queuedMessage(text, state, {uid, agent = null, className = ''} = {}) {
    const card = el('div', `msg queued-send ${className}`.trim());
    card.dataset.role = 'user';
    const body = el('div', 'mb');
    body.innerHTML = md(String(text || ''), true, [], {uid, agent, attachments:true});
    const foot = el('div', 'queued-send-foot'), status = el('span', 'queued-send-state', state);
    const actions = el('span', 'sdk-queue-actions');
    foot.append(status, actions); card.append(body, foot);
    return {card, body, foot, status, actions};
  },

  selectValue(select, value, label) {
    select.querySelector('option[value=""]')?.remove();
    if (value) {
      if (![...select.options].some(option => option.value === value)) select.append(new Option(value, value));
      select.value = value;
    } else {
      const placeholder = new Option(label, ''); placeholder.disabled = placeholder.hidden = true;
      select.prepend(placeholder); select.value = '';
    }
  },

  bindPopover(button, menu, scope) {
    // A conversation status button can be replaced while its menu survives.
    // Keep one listener on the menu and point it at the current trigger.
    menu._trigger = button;
    if (!menu._bound) {
      menu._bound = true;
      menu.addEventListener('beforetoggle', event =>
        menu._trigger?.setAttribute('aria-expanded', String(event.newState === 'open')));
    }
    button.onclick = event => {
      event.preventDefault();
      if (button.disabled) return;
      if (menu.matches(':popover-open')) { menu.hidePopover(); return; }
      this.closePopovers(scope);
      menu.showPopover();
      this.placePopover(menu, button);
    };
  },

  closePopovers(scope) {
    for (const menu of scope?.querySelectorAll('.sdk-toolbar-menu, .sdk-bubble-menu') || [])
      if (menu.matches(':popover-open')) menu.hidePopover();
  },

  placePopover(menu, button, preferredWidth = 300) {
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

  statusSummary(text, kind) {
    text = String(text || '');
    if ([...text].length <= 6) return text;
    const lead = text.split(/[：:，,。；;\s]/)[0];
    if (lead && [...lead].length <= 6) return lead;
    if (text.startsWith('不支持的命令')) return '命令不支持';
    if (text.startsWith('队列暂停')) return '队列暂停';
    return {blocked: '执行失败', retrying: '正在重试', working: '处理中',
      waiting: '等待处理', maintenance: '压缩中', connecting: '载入中', ready: '就绪'}[kind] || '状态详情';
  },

  updateStatusDisclosure(notice, details) {
    notice._statusDetails = details;
    if (notice._statusDisclosureFrame != null) return;
    notice._statusDisclosureFrame = requestAnimationFrame(() => {
      notice._statusDisclosureFrame = null;
      const latest = notice._statusDetails;
      if (notice.isConnected && latest?.isConnected) this.updateStatusDisclosureNow(notice, latest);
    });
  },

  updateStatusDisclosureNow(notice, details) {
    const full = details.textContent.trim(), summary = notice.textContent.trim();
    const available = !notice.hidden && !!full
      && (full !== summary || notice.scrollWidth > notice.clientWidth + 1);
    if (notice.disabled !== !available) notice.disabled = !available;
    if (available) {
      for (const [name, value] of Object.entries({'aria-controls': details.id,
          'aria-haspopup': 'dialog', 'aria-expanded': String(details.matches(':popover-open'))}))
        if (notice.getAttribute(name) !== value) notice.setAttribute(name, value);
    } else {
      if (details.matches(':popover-open')) details.hidePopover();
      for (const attribute of ['aria-controls', 'aria-haspopup', 'aria-expanded'])
        if (notice.hasAttribute(attribute)) notice.removeAttribute(attribute);
    }
  },

  refreshToolTails(box) {
    if (!box) return;
    const groups = box.querySelectorAll('.grp');
    if (!groups.length) return;
    // Use message order across history/live and turn wrappers, not execution state.
    const all = box.querySelectorAll('.msg, [data-sdk-item]');
    let last = null;
    for (let i = all.length - 1; i >= 0; i--) {
      if (!all[i].closest('.silent-tool-result, .question-live-shadowed, [hidden], .hidden')) {
        last = all[i];
        break;
      }
    }
    const tailGroup = last?.closest('.grp') || null;
    for (const group of groups) group._setTailAtEnd?.(group === tailGroup);
  },

  toolComplete(message) {
    if (['pending', 'in_progress', 'inProgress', 'running'].includes(message.status)) return false;
    return message.role === 'tool_result' || !!message.result
      || ['completed', 'failed', 'declined', 'cancelled', 'canceled'].includes(message.status);
  },

  toolState(node) {
    return node ? ['.tool-out-actions .more', '.tool-toggle', '.tool-wrap'].map(selector =>
      [...node.querySelectorAll(selector)].map(control =>
        control.getAttribute(selector === '.tool-wrap' ? 'aria-pressed' : 'aria-expanded'))) : null;
  },

  restoreToolState(node, state) {
    if (!state) return;
    ['.tool-out-actions .more', '.tool-toggle', '.tool-wrap'].forEach((selector, group) => {
      node.querySelectorAll(selector).forEach((control, index) => {
        const attribute = selector === '.tool-wrap' ? 'aria-pressed' : 'aria-expanded';
        const previous = state[group][index];
        if (previous != null && control.getAttribute(attribute) !== previous) control.click();
      });
    });
  },

  voiceEnabled() {
    if (typeof VoiceInput !== 'object' || !VoiceInput) return false;
    if (typeof SessionDockCapabilities === 'object' && SessionDockCapabilities) {
      if (SessionDockCapabilities.config?.read_only === true) return false;
      if (!SessionDockCapabilities.allows('voice_input')) return false;
    }
    return true;
  },

  voiceStoragePrefix() {
    if (typeof STORAGE_PREFIX === 'string' && STORAGE_PREFIX) return STORAGE_PREFIX;
    return (typeof SessionDockCapabilities === 'object' && SessionDockCapabilities?.namespace) || 'sessiondock.';
  },

  voiceProviders() {
    const list = (typeof SessionDockCapabilities === 'object' && SessionDockCapabilities?.config?.voice_providers) || null;
    return Array.isArray(list) && list.length ? list : null;
  },

  voiceModel() {
    try {
      const raw = SessionDockCapabilities.stored('voiceModel', this.voiceStoragePrefix());
      const parsed = raw ? JSON.parse(raw) : VoiceInput.DEFAULT_MODEL;
      return VoiceInput.validModelId(parsed, this.voiceProviders());
    } catch {
      return VoiceInput.validModelId(VoiceInput.DEFAULT_MODEL, this.voiceProviders());
    }
  },

  setVoiceModel(id) {
    try {
      localStorage.setItem(this.voiceStoragePrefix() + 'voiceModel', JSON.stringify(id));
    } catch {}
  },

  collectVoiceContext() {
    const sdkActive = typeof SessionContentView === 'object'
      && SessionContentView?.kind === 'sdk'
      && typeof SdkView === 'object'
      && SdkView?.box?.isConnected;
    const box = sdkActive ? SdkView.box : document.querySelector('#msgs');
    const messages = [];
    if (box && box.isConnected && !box.hidden && !box.classList.contains('hidden')) {
      const nodes = box.querySelectorAll('.msg');
      for (const node of nodes) {
        const role = node.dataset.role || '';
        if (role !== 'user' && role !== 'assistant' && role !== 'assistant·subagent') continue;
        if (node.classList.contains('tool-msg') || node.classList.contains('user-shell-command')) continue;
        if (node.closest('.turn-process, .turn-process-body, .grp, .tool-group, .sdk-tool-group, .silent-tool-result, [hidden], .hidden')) continue;
        const raw = typeof node._messageData?.text === 'string'
          ? node._messageData.text
          : (node.querySelector('.mb')?.innerText || '');
        const trimmed = String(raw || '').trim();
        if (!trimmed) continue;
        messages.push({
          role: role === 'user' ? 'user' : 'assistant',
          text: trimmed.length > 600 ? trimmed.slice(-600) : trimmed,
        });
      }
    }
    const continueRef = document.querySelector('#new-session-dialog[open] #continue-session-reference:not([hidden])')?.textContent?.trim() || '';
    const sessionTitle = document.querySelector('#a-session-title')?.textContent?.trim() || '';
    return {
      title: continueRef || sessionTitle,
      messages: messages.slice(-6),
    };
  },

  mountVoiceInput(input, buttonHost, {beforeElement = null, getUid = null} = {}) {
    if (!this.voiceEnabled() || !input || !buttonHost) return null;
    const resolveUidObj = () => {
      const uid = typeof getUid === 'function' ? String(getUid() || '').trim() : '';
      return uid ? {uid} : {};
    };
    return VoiceInput.attach({
      input,
      buttonHost,
      beforeElement,
      endpoint: '/api/voice/transcribe',
      realtimeEndpoint: '/api/voice/realtime',
      getProviders: () => this.voiceProviders(),
      getModel: () => this.voiceModel(),
      setModel: id => this.setVoiceModel(id),
      getContext: () => this.collectVoiceContext(),
      getRequestExtra: resolveUidObj,
      getRealtimeParams: resolveUidObj,
      getHeaders: () => {
        const headers = {};
        if (typeof AUDIT_PAGE_ID === 'string' && AUDIT_PAGE_ID) headers['X-SessionDock-Page'] = AUDIT_PAGE_ID;
        if (typeof BUILD_ID === 'string' && BUILD_ID) headers['X-SessionDock-Build'] = BUILD_ID;
        return headers;
      },
    });
  },

  mountStaticVoiceInputs() {
    if (!this.voiceEnabled()) return;
    const bindings = [
      {
        input: document.querySelector('#cinput'),
        host: document.querySelector('#composer .cbtns'),
        before: document.querySelector('#csend'),
        getUid: () => (typeof composerUid === 'string' && composerUid) || (typeof S === 'object' && S?.sel) || '',
      },
      {
        input: document.querySelector('#continue-session-description'),
        host: document.querySelector('#continue-session-composer .cbtns'),
        before: document.querySelector('#continue-session-go'),
        getUid: () => (typeof newContinueOrigin === 'object' && newContinueOrigin?.uid) || (typeof S === 'object' && S?.sel) || '',
      },
      {
        input: document.querySelector('#bug-report-description'),
        host: document.querySelector('#bug-report-form .report-composer .cbtns'),
        before: document.querySelector('#bug-report-go'),
        getUid: () => (typeof S === 'object' && S?.sel) || '',
      },
    ];
    for (const item of bindings) {
      if (!item.input || !item.host || item.host.querySelector('[data-voice-input]')) continue;
      this.mountVoiceInput(item.input, item.host, {
        beforeElement: item.before,
        getUid: item.getUid,
      });
    }
  },
};

ConversationUI.mountStaticVoiceInputs();
