'use strict';

// Goal state belongs to Codex. The panel only reads it and submits user actions.
const CodexSdkGoal = {
  states: new WeakMap(),
  names: {active:'进行中', paused:'已暂停', blocked:'受阻', usageLimited:'用量受限', budgetLimited:'预算用尽', complete:'已完成'},

  mount(view) {
    const state = {generation:view.generation, uid:view.uid, goal:null, loaded:false, busy:false, dirty:false, revision:0};
    this.states.set(view, state);
    const pickers = view.pane.querySelector('.sdk-pickers'), more = pickers.querySelector('#sdk-more');
    const compact = view.pane.querySelector('[data-sdk-action="compact"]');
    compact.classList.add('sdk-fixed-action');
    pickers.insertBefore(compact, more);
    const button = el('button', 'new-model-option sdk-action');
    button.id = 'sdk-goal'; button.type = 'button'; button.title = 'Goal 目标';
    button.setAttribute('aria-label', 'Goal 目标'); button.setAttribute('aria-expanded', 'false');
    button.setAttribute('aria-controls', 'sdk-goal-menu');
    button.innerHTML = uiIcon('goal');
    pickers.insertBefore(button, more);
    const panel = el('div', 'new-model-menu sdk-toolbar-menu sdk-goal-menu');
    panel.id = 'sdk-goal-menu'; panel.setAttribute('popover', 'auto');
    panel.setAttribute('role', 'dialog'); panel.setAttribute('aria-label', 'Goal 目标');
    panel.innerHTML = `<form class="sdk-goal-form">
      <div class="sdk-goal-heading"><h3>Goal</h3><span id="sdk-goal-state">读取中…</span></div>
      <p class="sdk-control-note" id="sdk-goal-usage"></p>
      <label>目标<textarea id="sdk-goal-objective" rows="4" required placeholder="希望 Codex 持续完成什么？"></textarea></label>
      <label>Token 预算<input id="sdk-goal-budget" type="number" min="1" step="1" placeholder="不设上限"></label>
      <p class="sdk-control-note sdk-goal-result" role="status"></p>
      <div class="sdk-goal-buttons">
        <button class="btn primary" type="submit" data-goal-action="save">设置目标</button>
        <button class="btn" type="button" data-goal-action="resume">恢复</button>
        <button class="btn" type="button" data-goal-action="pause">暂停</button>
        <button class="btn" type="button" data-goal-action="clear">清除</button>
      </div>
    </form>`;
    pickers.append(panel);
    state.panel = panel; state.button = button;
    state.resize = new ResizeObserver(() => this.place(state));
    state.resize.observe(panel);
    ConversationUI.bindPopover(button, panel, view.pane);
    const toggle = button.onclick;
    button.onclick = event => {
      toggle(event);
      if (panel.matches(':popover-open')) void this.load(view);
    };
    panel.querySelector('form').onsubmit = event => { event.preventDefault(); void this.save(view); };
    for (const input of panel.querySelectorAll('textarea, input')) input.oninput = () => { state.dirty = true; };
    for (const action of ['resume', 'pause', 'clear']) {
      panel.querySelector(`[data-goal-action="${action}"]`).onclick = () => void this.mutate(view, action);
    }
    this.paint(view);
  },

  close(view) { this.states.get(view)?.resize.disconnect(); this.states.delete(view); },
  current(view, state) { return state && this.states.get(view) === state && view.current(state.generation) && view.uid === state.uid; },
  result(state, text = '', error = false) {
    const node = state.panel.querySelector('.sdk-goal-result');
    node.textContent = text; node.classList.toggle('sdk-control-error', error);
  },
  place(state) {
    if (state.panel.matches(':popover-open')) ConversationUI.placePopover(state.panel, state.button, 360);
  },
  paint(view) {
    const state = this.states.get(view);
    if (!state) return;
    state.button.disabled = view.readOnly || !view.summary;
    const {panel, goal} = state;
    panel.querySelector('#sdk-goal-state').textContent = state.loaded ? (goal ? this.names[goal.status] || goal.status : '未设置') : state.loadError ? '读取失败' : '读取中…';
    panel.querySelector('#sdk-goal-usage').textContent = goal
      ? `已用 ${Number(goal.tokensUsed || 0).toLocaleString()} tokens · ${Math.floor((goal.timeUsedSeconds || 0) / 60)} 分钟`
      : '设置目标后，Codex 会在可继续时自动推进。';
    const unavailable = state.busy || !state.loaded || view.readOnly;
    for (const field of panel.querySelectorAll('textarea, input, button')) field.disabled = unavailable;
    const active = goal?.status === 'active', complete = goal?.status === 'complete';
    panel.querySelector('[data-goal-action="save"]').textContent = goal && !complete ? '保存修改' : '设置目标';
    panel.querySelector('[data-goal-action="resume"]').hidden = !goal || active || complete;
    panel.querySelector('[data-goal-action="pause"]').hidden = !active;
    panel.querySelector('[data-goal-action="clear"]').hidden = !goal;
    this.place(state);
  },
  accept(view, state, goal, reset = false) {
    if (!this.current(view, state)) return;
    state.goal = goal; state.loaded = true;
    if (reset) state.dirty = false;
    if (!state.dirty) {
      state.panel.querySelector('#sdk-goal-objective').value = goal?.objective || '';
      state.panel.querySelector('#sdk-goal-budget').value = goal?.tokenBudget ?? '';
    }
    this.paint(view);
  },
  async load(view) {
    const state = this.states.get(view);
    if (!state || state.busy) return;
    const revision = ++state.revision;
    state.busy = true; state.loadError = false; this.result(state); this.paint(view);
    try {
      const data = await view.request('api/codex-sdk/goal/get', {uid:state.uid});
      if (this.current(view, state) && revision === state.revision) this.accept(view, state, data.raw.goal);
    } catch (error) {
      if (this.current(view, state)) {
        state.loaded = false; state.loadError = true;
        this.result(state, `读取 Goal 失败：${error.message}`, true);
      }
    } finally {
      if (this.current(view, state)) { state.busy = false; this.paint(view); }
    }
  },
  async save(view) {
    const state = this.states.get(view);
    if (!state || state.busy || !state.loaded) return;
    const objective = state.panel.querySelector('#sdk-goal-objective').value.trim();
    const field = state.panel.querySelector('#sdk-goal-budget');
    if (!objective || !field.reportValidity()) return;
    const values = {objective, tokenBudget:field.value === '' ? null : Number(field.value)};
    if (!state.goal || state.goal.status === 'complete') values.status = 'active';
    await this.mutate(view, 'save', values);
  },
  async mutate(view, action, values = {}) {
    const state = this.states.get(view);
    if (!state || state.busy) return false;
    state.busy = true; ++state.revision; this.result(state); this.paint(view);
    try {
      const data = await view.request(`api/codex-sdk/goal/${action === 'clear' ? 'clear' : 'set'}`,
        {uid:state.uid, ...(action === 'save' ? values : action === 'clear' ? {} : {status:action === 'resume' ? 'active' : 'paused'})});
      if (!this.current(view, state)) return true;
      this.accept(view, state, action === 'clear' ? null : data.raw.goal, action === 'save' || action === 'clear');
      const message = {save:'目标已保存。', resume:'目标已恢复；后续执行由 Codex 调度。', pause:'目标已暂停。', clear:'目标已清除。'}[action];
      this.result(state, message); view.setNotice(message);
      return true;
    } catch (error) {
      if (this.current(view, state)) {
        this.result(state, `目标操作失败：${error.message}`, true);
        view.setNotice(`目标操作失败：${error.message}`, 'blocked');
      }
      return false;
    } finally {
      if (this.current(view, state)) { state.busy = false; this.paint(view); }
    }
  },
  updated(view, goal) {
    const state = this.states.get(view);
    if (!state) return;
    ++state.revision;
    this.accept(view, state, goal);
  },
  async command(view, text, argument) {
    if (!['', 'status', 'resume', 'pause', 'clear'].includes(argument)) return false;
    const state = this.states.get(view), input = view.input;
    if (!state) return false;
    if (state.busy) { view.setNotice('目标操作正在进行，请稍候。'); return true; }
    if (!argument || argument === 'status') {
      if (!state.panel.matches(':popover-open')) state.button.click();
      else await this.load(view);
      return true;
    }
    if (await this.mutate(view, argument) && this.current(view, state) && input === view.input && input.value === text) {
      input.value = ''; view.fitInput(); CodexSdkCompose.input(view);
    }
    return true;
  },
};
