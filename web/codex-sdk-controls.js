'use strict';

// Native app-server controls. Every mutation follows an explicit user action;
// current values come from the shared thread, including changes from its TUI.
const CodexSdkControls = {
  states: new WeakMap(),
  sandboxNames: {readOnly: '只读', workspaceWrite: '工作区写入', dangerFullAccess: '完全访问', externalSandbox: '外部沙箱'},
  approvalNames: {'on-request': '按需审批', untrusted: '不可信命令需审批', never: '不请求审批'},

  mount(view) {
    view.settingChoices = new Map(); view.nativeSettingValues = new Map();
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
    tier.innerHTML = '<span class="visually-hidden">服务档位</span><select id="sdk-tier" aria-label="服务档位"><option value="" disabled hidden>档位</option></select>';
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
      view.selectValue(mode, sdkSettingValue(view, 'mode', settings.collaborationMode?.mode || ''), '模式');
      mode.disabled = view.settingChoices.has('mode');
    }
    const dialog = state.panel;
    if (!dialog) return;
    for (const [kind, value, names] of [
      ['approval', typeof settings.approvalPolicy === 'string' ? settings.approvalPolicy : '', this.approvalNames],
      ['sandbox', settings.sandboxPolicy?.type || '', this.sandboxNames],
    ]) {
      const select = view.pane.querySelector(`#sdk-control-${kind}`);
      view.selectValue(select, sdkSettingValue(view, kind, value), '未返回');
      select.title = `${kind === 'approval' ? '审批策略' : '沙箱策略'}：${names[select.value] || select.value || '未返回'}`;
      select.disabled = !!state.saving || view.settingChoices.has(kind);
    }
    view.layoutToolbar();
    sdkSyncSettings(view);
  },

  paintTier(view) {
    const select = view.pane?.querySelector('#sdk-tier');
    if (!select) return;
    const settings = this.effective(view);
    const model = view.models.find(row => row.model === settings.model);
    // Native IDs can differ from their labels (e.g. priority is Fast).
    // Catalog defaults do not establish this thread's effective setting.
    const tiers = [{id: 'default', name: 'Normal', description: '普通服务档位'}, ...(model?.serviceTiers || [])];
    const value = sdkSettingValue(view, 'service_tier', Object.hasOwn(settings, 'serviceTier') ? (settings.serviceTier || 'default') : '');
    select.disabled = view.settingChoices.has('service_tier');
    const signature = JSON.stringify([tiers, value]);
    if (select.dataset.options !== signature) {
      select.replaceChildren();
      for (const tier of tiers) {
        if ([...select.options].some(option => option.value === tier.id)) continue;
        const option = new Option(tier.name, tier.id);
        option.title = tier.description || '';
        select.append(option);
      }
      view.selectValue(select, value, '档位');
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
    const choices = {};
    for (const key of ['model', 'effort']) if (key in values) choices[key] = values[key];
    if ('mode' in values) choices.mode = values.mode;
    if ('service_tier' in values) choices.service_tier = values.service_tier;
    if ('approval_policy' in values) choices.approval = values.approval_policy;
    if ('sandbox_policy' in values) choices.sandbox = values.sandbox_policy.type;
    await sdkUpdateSettings(view, choices, () => this.request(view, 'settings', values), () => {view.paintPickers(); this.paint(view);});
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
      const holder = document.createElement('label');
      holder.className = 'new-effort sdk-policy'; holder.title = label;
      const select = document.createElement('select');
      select.id = `sdk-control-${kind}`; select.setAttribute('aria-label', label);
      for (const [value, title] of Object.entries(names)) {
        select.append(new Option(title, value));
      }
      select.onchange = async () => {
        if (state.saving || !select.value) return;
        const value = select.value;
        state.saving = true; this.paint(view);
        await this.run(view, select, () => this.settings(view, kind === 'approval'
          ? {approval_policy: value} : {sandbox_policy: {type: value}}), '权限设置已提交。');
        if (!this.current(view, state)) return;
        state.saving = false; this.paint(view);
      };
      holder.append(select);
      const pickers = view.pane.querySelector('.sdk-pickers');
      pickers.insertBefore(holder, pickers.querySelector('#sdk-more'));
    }
    this.paint(view);
  },

  show(view) {
    const trigger = view.pane.querySelector('#sdk-control-approval');
    view.closeToolbarMenus();
    if (trigger.closest('#sdk-more-menu')) view.pane.querySelector('#sdk-more').click();
    trigger.focus();
    try { trigger.showPicker?.(); } catch { /* Focus still exposes the native keyboard picker. */ }
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

// A settings receipt can precede its authoritative snapshot. Keep the requested
// selection disabled until that snapshot confirms or normalizes it. Failures
// restore the last native value; detached views cannot receive late results.
function sdkSettingValue(view, key, current) {
  view.nativeSettingValues.set(key, current);
  const pending = view.settingChoices.get(key);
  if (!pending) return current;
  if (current === pending.value || (pending.original != null && pending.original !== '' && current !== pending.original)) pending.observed = true;
  if (pending.accepted && pending.observed) { view.settingChoices.delete(key); return current; }
  return pending.value;
}

async function sdkUpdateSettings(view, values, request, paint) {
  if (view.settingChoices.size) throw new Error('设置正在等待原生确认，请稍后再修改');
  const generation = view.generation, choices = view.settingChoices;
  const updates = Object.entries(values).map(([key, value]) => {
    const pending = {value, original:view.nativeSettingValues.get(key), accepted:false, observed:false};
    choices.set(key, pending); return [key, pending];
  });
  paint();
  try {
    await request();
    if (!view.current(generation)) return;
    for (const [, pending] of updates) pending.accepted = true;
  } catch (error) {
    for (const [key, pending] of updates) if (choices.get(key) === pending) choices.delete(key);
    throw error;
  } finally { if (view.current(generation)) paint(); }
}

function sdkSyncSettings(view) {
  if (!view.pane) return;
  const pending = view.settingChoices.size > 0;
  for (const select of view.pane.querySelectorAll('#sdk-model, #sdk-effort, #sdk-mode, #sdk-tier, #sdk-control-approval, #sdk-control-sandbox, select[id^="native-sdk-"]')) {
    select.disabled = pending;
    if (pending) select.setAttribute('aria-busy', 'true'); else select.removeAttribute('aria-busy');
  }
  const button = view.pane.querySelector('#sdk-send');
  button.disabled = pending || !!view.stopped || !!CodexSdkCompose.states.get(view)?.sending;
}
