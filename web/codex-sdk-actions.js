'use strict';

// Native conversation actions share the SDK composer and existing dialogs.
const CodexSdkActions = {
  dialog: null, pending: false,

  mount(view) {
    const holder = view.pane?.querySelector('.sdk-pickers');
    if (!holder) return;
    const label = document.createElement('label');
    label.className = 'new-effort sdk-actions';
    label.title = '会话操作';
    label.innerHTML = '<span class="visually-hidden">会话操作</span><select id="sdk-actions" aria-label="会话操作">'
      + '<option value="">会话操作…</option><option value="fork">分叉会话</option>'
      + '<option value="revert">回退对话</option><option value="compact">压缩上下文</option>'
      + '<option value="review">代码审查</option></select>';
    label.querySelector('select').onchange = event => {
      const action = event.target.value;
      event.target.value = '';
      if (action) void this.command(view, `/${action}`);
    };
    holder.append(label);
    this.paint(view);
  },

  close() {
    this.dialog?.close();
    this.dialog?.remove();
    this.dialog = null;
    this.pending = false;
  },

  paint(view) {
    const select = view.pane?.querySelector('#sdk-actions');
    if (select) select.disabled = this.pending || !view.summary;
  },

  // Returns true only for these exact slash commands; quoted prose is chat.
  async command(view, text) {
    const match = /^\/(fork|revert|compact|review)(?:\s+([\s\S]*))?$/.exec(text.trim());
    if (!match) return false;
    if (this.pending) { view.setNotice('会话操作正在进行，请稍候。'); return true; }
    const action = match[1], argument = (match[2] || '').trim();
    const uid = view.uid, generation = view.generation, input = view.input;
    const current = () => view.current(generation) && view.uid === uid;
    const operation = {};
    this.pending = operation;
    this.paint(view);
    let succeeded = false;
    try {
      if (action === 'fork') {
        const response = await view.request('api/codex-sdk/fork', {uid, last_turn_id: argument || null});
        if (!current()) return true;
        if (response.session) T.sdkSessions = [...T.sdkSessions.filter(row => row.uid !== response.uid), response.session];
        CodexSdkView.legacyViews.delete(response.uid);
        // Clear the source command before opening the new session (draft hooks).
        if (input?.value === text) {
          input.value = ''; view.fitInput();
          if (typeof CodexSdkCompose !== 'undefined') CodexSdkCompose.input(view);
        }
        // Native forks are normally persisted immediately, so hydrate their
        // inventory before opening instead of treating them as empty starts.
        if (!response.session?.sdk_pending) await loadSessions(true, true);
        await openSession(response.uid);
        void loadSessions(true, true);
        return true;
      }
      if (action === 'revert') {
        const turn = argument || await this.chooseRevert(view, uid);
        if (!turn || !current()) return true;
        if (argument && !await appConfirm(`回退对话到此轮之前？\n\n将移除 ${turn} 及之后的对话，仅修改会话历史，不回滚工作区文件。`, {ok:'回退'})) return true;
        if (!current()) return true;
        await view.request('api/codex-sdk/revert', {uid, before_turn_id:turn});
        if (current()) {
          view.turn = null; view.items.clear(); view.nodes.clear(); view.live.replaceChildren();
          await view.refreshHistory(generation, true);
          view.setNotice('对话已回退；工作区文件保持原样。');
        }
        succeeded = true;
      } else if (action === 'compact') {
        if (argument) throw new Error('用法：/compact（无需参数）');
        await view.request('api/codex-sdk/compact', {uid});
        if (current()) view.setNotice('已请求压缩上下文，进度将显示在会话中。');
        succeeded = true;
      } else if (action === 'review') {
        const target = argument ? this.reviewTarget(argument) : await this.chooseReview();
        if (!target || !current()) return true;
        await view.request('api/codex-sdk/review', {uid, target});
        if (current()) view.setNotice('已启动代码审查。');
        succeeded = true;
      }
      if (succeeded && current() && input?.value === text) {
        input.value = ''; view.fitInput();
        if (typeof CodexSdkCompose !== 'undefined') CodexSdkCompose.input(view);
      }
    } catch (error) {
      if (current()) view.setNotice(`会话操作失败：${error.message}`, 'blocked');
    } finally {
      if (this.pending === operation) this.pending = false;
      if (current()) this.paint(view);
    }
    return true;
  },

  reviewTarget(argument) {
    const match = /^(uncommitted|branch|commit|custom)(?:\s+([\s\S]*))?$/.exec(argument);
    const kind = match?.[1], detail = (match?.[2] || '').trim();
    if (kind === 'uncommitted' && !detail) return {type:'uncommittedChanges'};
    if (detail && kind === 'branch') return {type:'baseBranch', branch:detail};
    if (detail && kind === 'commit') return {type:'commit', sha:detail};
    if (detail && kind === 'custom') return {type:'custom', instructions:detail};
    throw new Error('用法：/review uncommitted，或 /review branch 分支，/review commit SHA，/review custom 审查要求');
  },

  form(title, note, ok) {
    this.dialog?.remove();
    const dialog = document.createElement('dialog');
    dialog.className = 'app-dialog';
    dialog.id = 'sdk-action-dialog';
    const form = document.createElement('form');
    form.className = 'report-form';
    form.innerHTML = '<div class="modal-head"><div><h2></h2></div><button type="button" class="modal-close" aria-label="关闭">×</button></div>'
      + '<p class="continue-session-reference"></p><div class="sdk-action-fields"></div>'
      + '<div class="app-popup-message" role="status"></div><div class="modal-actions">'
      + '<button type="button" class="btn" data-action="cancel">取消</button>'
      + '<button type="submit" class="btn primary" data-action="submit"></button></div>';
    form.querySelector('h2').textContent = title;
    form.querySelector('p').textContent = note;
    form.querySelector('[data-action="submit"]').textContent = ok;
    dialog.setAttribute('aria-label', title);
    dialog.append(form);
    document.body.append(dialog);
    this.dialog = dialog;
    dialog.showModal();
    let finish;
    const answer = new Promise(resolve => { finish = resolve; });
    dialog.addEventListener('close', () => { finish(null); dialog.remove(); if (this.dialog === dialog) this.dialog = null; });
    form.querySelector('.modal-close').onclick = form.querySelector('[data-action="cancel"]').onclick = () => dialog.close();
    return {dialog, form, fields:form.querySelector('.sdk-action-fields'), error:form.querySelector('[role="status"]'),
      answer, finish: value => { finish(value); dialog.close(); }};
  },

  async chooseRevert(view, uid) {
    const modal = this.form('回退对话', '选择要移除的首轮，该轮及之后的对话会被移除。仅回退会话历史，不回滚工作区文件。', '回退');
    const label = document.createElement('label');
    label.className = 'new-effort';
    const select = document.createElement('select');
    select.id = 'sdk-revert-turn'; select.setAttribute('aria-label', '回退轮次');
    label.append(select); modal.fields.append(label);
    const more = document.createElement('button');
    more.type = 'button'; more.className = 'btn'; more.textContent = '加载更早轮次'; more.hidden = true;
    modal.fields.append(more);
    const submit = modal.form.querySelector('[type="submit"]');
    submit.disabled = true;
    let cursor = null;
    const load = async () => {
      more.disabled = true;
      modal.error.textContent = '读取轮次…';
      try {
        const data = await view.request('api/codex-sdk/turns', {uid, cursor});
        if (!modal.dialog.isConnected) return;
        for (const turn of data.raw?.data || []) {
          // The summary's user item is supplied by the daemon, not guessed from disk.
          const message = (turn.items || []).find(item => item.type === 'userMessage');
          const preview = (message?.content || []).filter(item => item.type === 'text').map(item => item.text).join(' ');
          select.add(new Option(`${preview ? preview.slice(0, 64) + ' · ' : ''}${turn.id}`, turn.id));
        }
        cursor = data.raw?.nextCursor || null;
        more.hidden = !cursor;
        submit.disabled = !select.options.length;
        modal.error.textContent = select.options.length ? '' : '当前没有可回退的轮次。';
      } catch (error) { modal.error.textContent = `读取失败：${error.message}`; more.hidden = false; }
      finally { more.disabled = false; }
    };
    more.onclick = load;
    modal.form.onsubmit = event => { event.preventDefault(); if (select.value) modal.finish(select.value); };
    void load();
    return modal.answer;
  },

  async chooseReview() {
    const modal = this.form('代码审查', '选择审查范围，审查结果会显示在当前会话中。', '开始审查');
    modal.fields.innerHTML = '<label class="new-effort"><select id="sdk-review-target" aria-label="审查范围">'
      + '<option value="uncommitted">未提交的更改</option><option value="branch">与基准分支比较</option>'
      + '<option value="commit">指定提交</option><option value="custom">自定义审查要求</option></select></label>'
      + '<input id="sdk-review-detail" class="new-model-search" aria-label="审查参数" hidden>';
    const kind = modal.fields.querySelector('select'), detail = modal.fields.querySelector('input');
    kind.onchange = () => {
      detail.hidden = kind.value === 'uncommitted';
      detail.placeholder = {branch:'基准分支，例如 main', commit:'提交 SHA', custom:'希望重点审查什么？'}[kind.value] || '';
      detail.required = !detail.hidden;
    };
    modal.form.onsubmit = event => {
      event.preventDefault();
      try { modal.finish(this.reviewTarget(`${kind.value} ${detail.hidden ? '' : detail.value}`.trim())); }
      catch (error) { modal.error.textContent = error.message; }
    };
    return modal.answer;
  },
};
