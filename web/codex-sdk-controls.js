'use strict';

// Native app-server controls. Every mutation follows an explicit user action;
// current values come from the shared thread, including changes from its TUI.
const CodexSdkControls = {
  states: new WeakMap(),
  sandboxNames: {readOnly: '只读', workspaceWrite: '工作区写入', dangerFullAccess: '完全访问', externalSandbox: '外部沙箱'},
  approvalNames: {'on-request': '按需审批', untrusted: '不可信命令需审批', never: '不请求审批'},

  mount(view) {
    const state = {uid: view.uid, generation: view.generation, panel: null};
    this.states.set(view, state);
    const pickers = view.pane.querySelector('.sdk-pickers');
    const label = document.createElement('label');
    label.className = 'new-effort sdk-mode'; label.title = '协作模式';
    label.innerHTML = '<span class="visually-hidden">协作模式</span><select id="sdk-mode" aria-label="协作模式"><option value="" disabled hidden>模式</option><option value="default">执行模式</option><option value="plan">计划模式</option></select>';
    label.querySelector('select').onchange = async event => {
      const mode = event.target.value;
      if (!mode) return;
      event.target.disabled = true;
      try { await this.settings(view, {mode}); }
      catch (error) { if (this.current(view, state)) view.setNotice(`切换模式失败：${error.message}`, 'blocked', true); }
      finally { if (this.current(view, state)) { event.target.disabled = false; this.paint(view); } }
    };
    pickers.insertBefore(label, pickers.querySelector('#sdk-more'));
    const tier = document.createElement('label');
    tier.className = 'new-effort sdk-tier'; tier.title = '服务档位';
    tier.innerHTML = '<span class="visually-hidden">服务档位</span><select id="sdk-tier" aria-label="服务档位"><option value="" disabled hidden>Tier</option></select>';
    tier.querySelector('select').onchange = async event => {
      const service_tier = event.target.value;
      if (!service_tier) return;
      event.target.disabled = true;
      this.paintTier(view);
      try { await this.settings(view, {service_tier}); }
      catch (error) { if (this.current(view, state)) view.setNotice(`切换服务档位失败：${error.message}`, 'blocked', true); }
      finally { if (this.current(view, state)) { event.target.disabled = false; this.paintTier(view); } }
    };
    pickers.insertBefore(tier, label);
    this.mountPermissions(view, state);
  },

  close(view) {
    this.states.delete(view);
  },

  current(view, state) {
    return this.states.get(view) === state && view.uid === state.uid && view.current(state.generation);
  },

  effective(view) {
    const settings = view.summary?.settings || {}, resume = view.summary?.resume || {};
    return {...resume, ...settings, sandboxPolicy: settings.sandboxPolicy || settings.sandbox || resume.sandbox};
  },

  paint(view) {
    const state = this.states.get(view);
    if (!state || !view.pane) return;
    this.paintTier(view);
    const settings = this.effective(view);
    const mode = view.pane.querySelector('#sdk-mode');
    if (mode) {
      view.selectValue(mode, settings.collaborationMode?.mode || '', '模式');
    }
    const dialog = state.panel;
    if (!dialog) return;
    for (const [kind, value, names] of [
      ['approval', typeof settings.approvalPolicy === 'string' ? settings.approvalPolicy : '', this.approvalNames],
      ['sandbox', settings.sandboxPolicy?.type || '', this.sandboxNames],
    ]) {
      const trigger = view.pane.querySelector(`#sdk-control-${kind}`);
      trigger.querySelector('small').textContent = names[value] || '未返回';
      trigger.title = `${kind === 'approval' ? '审批策略' : '沙箱策略'}：${names[value] || '未返回'}`;
      trigger.setAttribute('aria-label', trigger.title);
      for (const button of view.pane.querySelectorAll(`[data-policy="${kind}"]`)) {
        const checked = button.dataset.value === value;
        button.setAttribute('aria-checked', String(checked));
        button.querySelector('.group-menu-check').textContent = checked ? '✓' : '';
        button.disabled = !!state.saving;
      }
    }
    view.layoutToolbar();
  },

  paintTier(view) {
    const select = view.pane?.querySelector('#sdk-tier');
    if (!select) return;
    const settings = this.effective(view);
    const model = view.models.find(row => row.model === settings.model);
    // Native IDs can differ from their labels (e.g. priority is Fast).
    // Catalog defaults do not establish this thread's effective setting.
    const tiers = [{id: 'default', name: 'Normal', description: '普通服务档位'}, ...(model?.serviceTiers || [])];
    const value = Object.hasOwn(settings, 'serviceTier') ? (settings.serviceTier || 'default') : '';
    const signature = JSON.stringify([tiers, value]);
    if (select.dataset.options !== signature) {
      select.replaceChildren();
      for (const tier of tiers) {
        if ([...select.options].some(option => option.value === tier.id)) continue;
        const option = new Option(tier.name, tier.id);
        option.title = tier.description || '';
        select.append(option);
      }
      view.selectValue(select, value, 'Tier');
      select.dataset.options = signature;
      view.toolbarLayout = null;
      view.layoutToolbar();
    } else select.value = value;
    select.title = `服务档位：${select.selectedOptions[0]?.textContent || '未返回'}`;
  },

  async request(view, action, body) {
    return view.request(`api/codex-sdk/controls/${action}`, {uid: view.uid, ...body});
  },

  async settings(view, values) {
    await this.request(view, 'settings', values);
    // Effective values are delivered by thread/settings/updated. No guessed
    // optimistic state: an app-server may reject or normalize a policy.
  },

  error(dialog, text) {
    const node = dialog.querySelector('.sdk-control-error');
    node.textContent = text; node.hidden = !text;
  },

  async run(view, button, operation, success) {
    const state = this.states.get(view), dialog = state?.panel;
    button.disabled = true; button.setAttribute('aria-busy', 'true');
    if (dialog) this.error(dialog, '');
    try {
      await operation();
      if (!this.current(view, state)) return;
      if (dialog?.isConnected) dialog.querySelector('#sdk-controls-result').textContent = success;
      view.setNotice(success, '', true);
    } catch (error) {
      if (this.current(view, state) && dialog?.isConnected) { this.error(dialog, error.message); view.setNotice(error.message, 'blocked', true); }
    } finally {
      if (this.current(view, state)) { button.disabled = false; button.removeAttribute('aria-busy'); this.paint(view); }
    }
  },

  mountPermissions(view, state) {
    const dialog = document.createElement('section');
    dialog.id = 'sdk-permissions'; dialog.className = 'sdk-control-section';
    dialog.setAttribute('aria-label', '权限');
    dialog.innerHTML = '<p id="sdk-controls-result" class="sdk-control-note" role="status"></p><p class="sdk-control-note sdk-control-error" role="alert" hidden></p>';
    state.panel = dialog;
    view.pane.querySelector('#sdk-more-menu').append(dialog);
    for (const [kind, label, names] of [
      ['approval', '审批策略', this.approvalNames],
      ['sandbox', '沙箱策略', {readOnly: '只读', workspaceWrite: '工作区写入', dangerFullAccess: '完全访问'}],
    ]) {
      const trigger = document.createElement('button');
      trigger.id = `sdk-control-${kind}`; trigger.type = 'button'; trigger.className = 'btn sdk-policy';
      trigger.setAttribute('aria-haspopup', 'menu'); trigger.setAttribute('aria-expanded', 'false');
      trigger.innerHTML = `<span class="sdk-policy-label">${kind === 'approval' ? '审批' : '沙箱'}</span><small></small><span class="sdk-policy-arrow" aria-hidden="true">▾</span>`;
      const menu = document.createElement('div');
      menu.id = `sdk-policy-${kind}`; menu.className = 'new-model-menu sdk-toolbar-menu';
      menu.setAttribute('popover', 'auto'); menu.setAttribute('role', 'menu'); menu.setAttribute('aria-label', label);
      trigger.setAttribute('aria-controls', menu.id);
      menu.addEventListener('beforetoggle', event => trigger.setAttribute('aria-expanded', String(event.newState === 'open')));
      trigger.onclick = () => {
        if (menu.matches(':popover-open')) { menu.hidePopover(); return; }
        view.closeToolbarMenus();
        menu.showPopover(); view.placeMenu(menu, trigger);
        menu.querySelector('[aria-checked="true"]')?.focus();
      };
      for (const [value, title] of Object.entries(names)) {
        const button = document.createElement('button');
        button.type = 'button'; button.className = 'new-model-option';
        button.dataset.policy = kind; button.dataset.value = value;
        button.setAttribute('role', 'menuitemradio');
        button.innerHTML = `<span class="group-menu-check" aria-hidden="true"></span><span>${title}</span>`;
        button.onclick = async () => {
          if (state.saving) return;
          state.saving = true; this.paint(view);
          await this.run(view, button, () => this.settings(view, kind === 'approval'
            ? {approval_policy: value} : {sandbox_policy: {type: value}}), '权限设置已应用。');
          if (!this.current(view, state)) return;
          state.saving = false; this.paint(view);
        };
        menu.append(button);
      }
      menu.onkeydown = event => {
        const buttons = [...menu.querySelectorAll('button:not(:disabled)')];
        const index = buttons.indexOf(document.activeElement);
        if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
          event.preventDefault(); buttons[(index + (event.key === 'ArrowDown' ? 1 : buttons.length - 1)) % buttons.length]?.focus();
        }
      };
      const pickers = view.pane.querySelector('.sdk-pickers');
      pickers.insertBefore(trigger, pickers.querySelector('#sdk-more'));
      view.pane.querySelector('.sdk-status-row').append(menu);
    }
    this.paint(view);
  },

  show(view) {
    const trigger = view.pane.querySelector('#sdk-control-approval');
    if (trigger.getAttribute('aria-expanded') !== 'true') trigger.click();
    view.pane.querySelector('#sdk-policy-approval [aria-checked="true"]')?.focus();
  },

  async command(view, text) {
    const state = this.states.get(view), input = view.input;
    const handled = await this.execute(view, text);
    if (handled && this.current(view, state) && view.input === input && input.value === text) {
      input.value = ''; view.fitInput();
      if (typeof CodexSdkCompose !== 'undefined') CodexSdkCompose.input(view);
    }
    return handled;
  },

  async execute(view, text) {
    const value = text.trim();
    if (value.startsWith('!')) {
      const command = value.slice(1).trim();
      if (!command) throw new Error('用法：!命令 或 /shell 命令');
      else await this.request(view, 'shell', {command});
      return true;
    }
    const match = /^\/([a-z-]+)(?:\s+([\s\S]*))?$/i.exec(value);
    if (!match) return false;
    const [, name, argument = ''] = match;
    switch (name.toLowerCase()) {
      case 'plan': case 'default':
        if (argument) throw new Error(`/${name} 不接受文本参数；切换模式后再发送消息。`);
        await this.settings(view, {mode: name.toLowerCase()}); return true;
      case 'permissions':
        if (argument) throw new Error('/permissions 用于打开权限设置；也可使用 /sandbox 或 /approval。');
        this.show(view); return true;
      case 'sandbox': {
        const type = {'read-only': 'readOnly', 'workspace-write': 'workspaceWrite', 'danger-full-access': 'dangerFullAccess', 'full-access': 'dangerFullAccess'}[argument];
        if (!type) throw new Error('用法：/sandbox read-only | workspace-write | danger-full-access');
        await this.settings(view, {sandbox_policy: {type}}); return true;
      }
      case 'approval':
        if (!(argument in this.approvalNames)) throw new Error('用法：/approval on-request | untrusted | never');
        await this.settings(view, {approval_policy: argument}); return true;
      case 'shell':
        if (!argument) throw new Error('用法：/shell 命令（或 !命令）');
        else await this.request(view, 'shell', {command: argument});
        return true;
      default: return false;
    }
  },
};
