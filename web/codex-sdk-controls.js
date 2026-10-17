'use strict';

// Native app-server controls. Every mutation follows an explicit user action;
// current values come from the shared thread, including changes from its TUI.
const CodexSdkControls = {
  states: new WeakMap(),
  statuses: {active: '进行中', paused: '已暂停', blocked: '受阻', usageLimited: '额度受限', budgetLimited: '预算用尽', complete: '已完成'},
  sandboxNames: {readOnly: '只读', workspaceWrite: '工作区写入', dangerFullAccess: '完全访问', externalSandbox: '外部沙箱'},
  approvalNames: {'on-request': '按需审批', untrusted: '不可信命令需审批', never: '不请求审批'},

  mount(view) {
    const state = {uid: view.uid, generation: view.generation, goal: undefined, goalRevision: 0, readAttempted: false, loading: false, dialog: null};
    this.states.set(view, state);
    const pickers = view.pane.querySelector('.sdk-pickers');
    const label = document.createElement('label');
    label.className = 'new-effort'; label.title = '协作模式';
    label.innerHTML = '<span class="visually-hidden">协作模式</span><select id="sdk-mode" aria-label="协作模式"><option value="">读取模式…</option><option value="default">Default</option><option value="plan">Plan</option></select>';
    const button = document.createElement('button');
    button.className = 'btn'; button.id = 'sdk-controls'; button.type = 'button';
    button.textContent = '会话'; button.title = '权限与目标';
    button.onclick = () => this.dialog(view);
    label.querySelector('select').onchange = async event => {
      const mode = event.target.value;
      if (!mode) return;
      event.target.disabled = true;
      try { await this.settings(view, {mode}); }
      catch (error) { if (this.current(view, state)) view.setNotice(`切换模式失败：${error.message}`, 'blocked', true); }
      finally { if (this.current(view, state)) { event.target.disabled = false; this.paint(view); } }
    };
    pickers.append(label, button);
  },

  close(view) {
    const state = this.states.get(view);
    if (state?.dialog) { state.dialog.close(); state.dialog.remove(); }
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
    const settings = this.effective(view);
    const mode = view.pane.querySelector('#sdk-mode');
    if (mode) {
      mode.value = settings.collaborationMode?.mode || '';
      mode.options[0].textContent = view.summary ? '当前模式未返回' : '读取模式…';
    }
    const button = view.pane.querySelector('#sdk-controls');
    if (button) button.title = state.goal ? `目标：${this.goalText(state.goal)}` : '权限与目标';
    if (view.summary && !state.readAttempted && !state.loading) this.refreshGoal(view);
    const dialog = state.dialog;
    if (!dialog) return;
    dialog.querySelector('#sdk-goal-current').textContent = state.goal === undefined ? (state.goalError || '正在读取目标…') : this.goalText(state.goal);
    dialog.querySelector('#sdk-goal-refresh').disabled = state.loading;
    const approval = dialog.querySelector('#sdk-control-approval');
    approval.options[0].textContent = `当前：${this.approvalNames[settings.approvalPolicy] || (settings.approvalPolicy ? '自定义审批' : '未返回')}`;
    const sandbox = dialog.querySelector('#sdk-control-sandbox');
    sandbox.options[0].textContent = `当前：${this.sandboxNames[settings.sandboxPolicy?.type] || '未返回'}`;
    const native = dialog.querySelector('#sdk-permission-current');
    native.textContent = settings.activePermissionProfile?.id ? `权限配置：${settings.activePermissionProfile.id}` : '';
    const goalButton = dialog.querySelector('#sdk-goal-save');
    goalButton.disabled = state.goal === undefined || goalButton.getAttribute('aria-busy') === 'true';
    const clear = dialog.querySelector('#sdk-goal-clear');
    clear.disabled = !state.goal || clear.getAttribute('aria-busy') === 'true';
  },

  event(view, method, raw) {
    const state = this.states.get(view);
    if (!state) return;
    if (method === 'thread/goal/updated') state.goal = raw.goal || null;
    else if (method === 'thread/goal/cleared') state.goal = null;
    else return;
    state.goalRevision++; state.goalError = '';
    this.paint(view);
  },

  goalText(goal) {
    if (!goal) return '尚未设置目标';
    const budget = goal.tokenBudget != null ? ` / ${goal.tokenBudget}` : '';
    return `${this.statuses[goal.status] || goal.status} · ${goal.tokensUsed || 0}${budget} tokens · ${goal.objective}`;
  },

  async request(view, action, body) {
    return view.request(`api/codex-sdk/controls/${action}`, {uid: view.uid, ...body});
  },

  async refreshGoal(view) {
    const state = this.states.get(view);
    if (!state || state.loading) return;
    state.loading = true; state.readAttempted = true; state.goalError = '';
    const revision = state.goalRevision;
    try {
      const response = await this.request(view, 'goal', {action: 'read'});
      if (!this.current(view, state)) return;
      // An event received while the read was in flight is newer than that read.
      if (state.goalRevision === revision) state.goal = response.raw?.goal || null;
      this.fillGoal(view);
    } catch (error) {
      if (!this.current(view, state)) return;
      state.goalError = `读取目标失败：${error.message}`;
      if (state.dialog) this.error(state.dialog, state.goalError);
    } finally {
      if (this.current(view, state)) { state.loading = false; this.paint(view); }
    }
  },

  fillGoal(view) {
    const state = this.states.get(view), dialog = state?.dialog;
    if (!dialog || dialog.dataset.goalEdited === '1') return;
    dialog.querySelector('#sdk-goal-objective').value = state.goal?.objective || '';
    dialog.querySelector('#sdk-goal-budget').value = state.goal?.tokenBudget ?? '';
    const status = dialog.querySelector('#sdk-goal-status');
    const actual = state.goal?.status || 'active';
    if (![...status.options].some(option => option.value === actual)) status.add(new Option(this.statuses[actual] || actual, actual));
    status.value = actual;
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
    const state = this.states.get(view), dialog = state?.dialog;
    button.disabled = true; button.setAttribute('aria-busy', 'true');
    if (dialog) this.error(dialog, '');
    try {
      await operation();
      if (!this.current(view, state)) return;
      if (dialog?.isConnected) dialog.querySelector('#sdk-controls-result').textContent = success;
    } catch (error) {
      if (this.current(view, state) && dialog?.isConnected) this.error(dialog, error.message);
    } finally {
      if (this.current(view, state)) { button.disabled = false; button.removeAttribute('aria-busy'); this.paint(view); }
    }
  },

  dialog(view, section = '') {
    const state = this.states.get(view);
    if (!state) return;
    if (state.dialog) { state.dialog.querySelector(section === 'goal' ? '#sdk-goal-objective' : '#sdk-control-approval').focus(); return; }
    const dialog = document.createElement('dialog');
    dialog.id = 'sdk-controls-dialog'; dialog.className = 'app-dialog app-popup';
    dialog.setAttribute('aria-labelledby', 'sdk-controls-title');
    dialog.innerHTML = `<div class="app-popup-panel">
      <div class="modal-head"><div><h2 id="sdk-controls-title">会话设置</h2></div><button class="modal-close" type="button" aria-label="关闭">×</button></div>
      <div class="sdk-control-body">
        <section class="sdk-control-section"><h3>权限</h3>
          <div class="sdk-control-row">
            <label class="new-effort"><span class="visually-hidden">审批策略</span><select id="sdk-control-approval" aria-label="审批策略"><option value="">读取审批策略…</option><option value="on-request">按需审批</option><option value="untrusted">不可信命令需审批</option><option value="never">不请求审批</option></select></label>
            <label class="new-effort"><span class="visually-hidden">沙箱策略</span><select id="sdk-control-sandbox" aria-label="沙箱策略"><option value="">读取沙箱策略…</option><option value="readOnly">只读</option><option value="workspaceWrite">工作区写入</option><option value="dangerFullAccess">完全访问</option></select></label>
          </div><p id="sdk-permission-current" class="sdk-control-note"></p>
          <div class="modal-actions"><button class="btn" id="sdk-permission-save" type="button">应用权限</button></div>
        </section>
        <section class="sdk-control-section"><div class="sdk-control-row"><h3>目标</h3><button class="btn" id="sdk-goal-refresh" type="button">刷新目标</button></div><p id="sdk-goal-current" class="sdk-control-note" role="status"></p>
          <label class="sdk-control-field">目标内容<textarea id="sdk-goal-objective" rows="2" placeholder="设置本会话要完成的目标"></textarea></label>
          <label class="sdk-control-field">Token 预算<input id="sdk-goal-budget" type="number" min="1" step="1" placeholder="留空沿用预算；新目标不限额"></label>
          <div class="sdk-control-row"><label class="new-effort"><span class="visually-hidden">目标状态</span><select id="sdk-goal-status" aria-label="目标状态"><option value="active">进行中</option><option value="paused">暂停</option><option value="complete">完成</option></select></label>
            <button class="btn" id="sdk-goal-clear" type="button">清除目标</button><button class="btn" id="sdk-goal-save" type="button">保存目标</button></div>
        </section>
        <p id="sdk-controls-result" class="sdk-control-note" role="status"></p><p class="sdk-control-note sdk-control-error" role="alert" hidden></p>
      </div></div>`;
    state.dialog = dialog;
    const close = () => { dialog.close(); dialog.remove(); if (state.dialog === dialog) state.dialog = null; };
    dialog.querySelector('.modal-close').onclick = close;
    dialog.addEventListener('cancel', event => { event.preventDefault(); close(); });
    for (const selector of ['#sdk-goal-objective', '#sdk-goal-budget', '#sdk-goal-status']) dialog.querySelector(selector).addEventListener('input', () => { dialog.dataset.goalEdited = '1'; });
    dialog.querySelector('#sdk-goal-refresh').onclick = () => this.refreshGoal(view);
    dialog.querySelector('#sdk-permission-save').onclick = event => this.run(view, event.target, async () => {
      const approval = dialog.querySelector('#sdk-control-approval'), sandbox = dialog.querySelector('#sdk-control-sandbox');
      const values = {};
      if (approval.value) values.approval_policy = approval.value;
      if (sandbox.value) values.sandbox_policy = {type: sandbox.value};
      if (!Object.keys(values).length) throw new Error('请先选择要修改的权限。');
      await this.settings(view, values);
      if (this.current(view, state)) { approval.value = ''; sandbox.value = ''; }
    }, '权限设置已应用；正在进行的回合保持原策略。');
    dialog.querySelector('#sdk-goal-save').onclick = event => this.run(view, event.target, async () => {
      const objective = dialog.querySelector('#sdk-goal-objective').value.trim();
      if (!objective) throw new Error('请输入目标内容。');
      const budgetInput = dialog.querySelector('#sdk-goal-budget');
      if (!budgetInput.checkValidity()) throw new Error('Token 预算须为正整数。');
      const values = {action: 'set', objective, status: dialog.querySelector('#sdk-goal-status').value};
      if (budgetInput.value) values.token_budget = Number(budgetInput.value);
      const response = await this.request(view, 'goal', values);
      if (this.current(view, state)) { state.goal = response.raw?.goal || null; dialog.dataset.goalEdited = ''; this.fillGoal(view); }
    }, '目标已保存。');
    dialog.querySelector('#sdk-goal-clear').onclick = event => this.run(view, event.target, async () => {
      await this.request(view, 'goal', {action: 'clear'});
      if (this.current(view, state)) { state.goal = null; dialog.dataset.goalEdited = ''; this.fillGoal(view); }
    }, '目标已清除。');
    document.body.append(dialog); this.paint(view); this.fillGoal(view); dialog.showModal();
    if (state.goalError) this.error(dialog, state.goalError);
    dialog.querySelector(section === 'goal' ? '#sdk-goal-objective' : '#sdk-control-approval').focus();
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
        this.dialog(view); return true;
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
      case 'goal': {
        const state = this.states.get(view);
        if (!argument || argument === 'status') { this.dialog(view, 'goal'); return true; }
        let values;
        if (argument === 'clear') values = {action: 'clear'};
        else if (['pause', 'resume', 'complete'].includes(argument)) values = {action: 'set', status: {pause: 'paused', resume: 'active', complete: 'complete'}[argument]};
        else values = {action: 'set', objective: argument, status: 'active'};
        const response = await this.request(view, 'goal', values);
        if (this.current(view, state)) { state.goal = response.raw?.goal || null; this.paint(view); this.fillGoal(view); }
        return true;
      }
      default: return false;
    }
  },
};
