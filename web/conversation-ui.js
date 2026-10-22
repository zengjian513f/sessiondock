'use strict';

// Presentation shared by SDK and CLI-backed conversation views. Callers own
// transport, draft identity and delivery evidence; these helpers never send.
const ConversationUI = {
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
    body.innerHTML = md(String(text || ''), true, [], {uid, agent});
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
      waiting: '等待处理', maintenance: '压缩中', connecting: '连接中', ready: '就绪'}[kind] || '状态详情';
  },

  updateStatusDisclosure(notice, details) {
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
  },

  refreshTools(box) {
    const entries = [...box.querySelectorAll('.tool-disclosable')]
      .filter(entry => !entry.closest('.user-shell-command'));
    const last = entries.at(-1);
    for (const entry of entries) {
      const open = entry.dataset.toolChoice ? entry.dataset.toolChoice === 'open' : entry === last;
      if (entry.classList.contains('tool-collapsed') === open) entry._setToolOpen(open);
    }
  },

  toolState(node) {
    return node ? ['.tool-out-actions .more', '.tool-toggle', '.tool-wrap'].map(selector =>
      [...node.querySelectorAll(selector)].map(control =>
        selector === '.tool-toggle' && !control.closest('.tool-entry')?.dataset.toolChoice ? null
          : control.getAttribute(selector === '.tool-wrap' ? 'aria-pressed' : 'aria-expanded'))) : null;
  },

  restoreToolState(node, state) {
    if (!state) return;
    ['.tool-out-actions .more', '.tool-toggle', '.tool-wrap'].forEach((selector, group) => {
      node.querySelectorAll(selector).forEach((control, index) => {
        const attribute = selector === '.tool-wrap' ? 'aria-pressed' : 'aria-expanded';
        const previous = state[group][index];
        if (previous != null) {
          if (control.getAttribute(attribute) !== previous) control.click();
          if (selector === '.tool-toggle') control.closest('.tool-entry').dataset.toolChoice = previous === 'true' ? 'open' : 'closed';
        }
      });
    });
  },
};
