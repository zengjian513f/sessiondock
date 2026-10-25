'use strict';

// Native conversation actions share the SDK composer and existing dialogs.
const CodexSdkActions = {
  dialog: null, pending: false,

  mount(view) {
    const holder = view.pane?.querySelector('#sdk-more-menu');
    if (!holder) return;
    const actions = document.createElement('div');
    actions.id = 'sdk-actions'; actions.setAttribute('role', 'group');
    actions.setAttribute('aria-label', '会话操作');
    for (const [action, text] of [['steer', '引导当前任务'], ['fork', '分支会话'], ['revert', '回退对话'], ['compact', '压缩上下文'], ['review', '代码审查']]) {
      const button = document.createElement('button');
      button.type = 'button'; button.className = 'new-model-option sdk-action';
      button.dataset.sdkAction = action; button.title = text; button.setAttribute('aria-label', text);
      button.innerHTML = uiIcon({steer:'continue', fork:'fork', revert:'undo', compact:'compact', review:'review'}[action]);
      const label = document.createElement('span'); label.className = 'sdk-action-label'; label.textContent = text;
      button.append(label);
      button.onclick = () => {
        view.closeToolbarMenus();
        if(action==='steer')void view.send('steer');
        else void this.command(view, `/${action}`);
      };
      actions.append(button);
    }
    holder.append(actions);
    this.paint(view);
  },

  close() {
    this.dialog?.close();
    this.dialog?.remove();
    this.dialog = null;
    this.pending = false;
  },

  paint(view) {
    CodexSdkGoal.paint(view);
    for (const button of view.pane?.querySelectorAll('[data-sdk-action]') || [])
      button.disabled = !view.summary || !!this.pending;
  },

  // Returns true only for these exact slash commands; quoted prose is chat.
  async command(view, text) {
    const match = /^\/(fork|revert|compact|review|rename|goal)(?:\s+([\s\S]*))?$/.exec(text.trim());
    if (!match) return false;
    if (match[1] === 'goal') return CodexSdkGoal.command(view, text, (match[2] || '').trim());
    if (this.pending) { view.setNotice('会话操作正在进行，请稍候。'); return true; }
    const action = match[1], argument = (match[2] || '').trim();
    const uid = view.uid, generation = view.generation, input = view.input;
    const current = () => view.current(generation) && view.uid === uid;
    const operation = {};
    this.pending = operation;
    this.paint(view);
    let succeeded = false;
    const requestId = crypto.randomUUID();
    const renameAudit = (phase,data={}) => { if(action==='rename') browserAuditEvent('sdk.rename.'+phase,{source:'codex',...data},{uid,requestId}); };
    renameAudit('invoked',{dialog:!argument});
    try {
      if (action === 'fork') {
        const response = await view.request('api/codex-sdk/fork', {uid, last_turn_id: argument || null});
        if (!current()) return true;
        // Clear the source command before opening the new session (draft hooks).
        if (input?.value === text) {
          input.value = ''; view.fitInput();
          if (typeof CodexSdkCompose !== 'undefined') CodexSdkCompose.input(view);
        }
        await this.openFork(view, response);
        return true;
      }
      if (action === 'rename') {
        let name = argument;
        if (!name) {
          const modal = this.form('重命名会话', '输入此 Codex 会话的新名称。', '保存');
          const field = document.createElement('input');
          field.className = 'new-model-search'; field.setAttribute('aria-label', '会话名称'); field.required = true;
          field.value = view.summary?.resume?.thread?.name || '';
          modal.fields.append(field);
          modal.form.onsubmit = event => { event.preventDefault(); if (field.value.trim()) modal.finish(field.value.trim()); };
          field.focus(); field.select();
          name = await modal.answer;
        }
        if (!name || !current()) { renameAudit('finished',{outcome:name?'session_changed':'cancelled'}); return true; }
        renameAudit('requested',{title_length:name.length});
        await view.request('api/codex-sdk/rename', {uid, name, request_id:requestId});
        renameAudit('finished',{outcome:'succeeded'});
        succeeded = true;
        if (current()) view.setNotice('会话已重命名。');
        await loadTermList(true);
        await loadSessions(true, true);
        const pending = view.pending(uid);
        if (current() && !view.agent && pending) {
          document.querySelector('#detail > .dhead')?.replaceWith(head(pending, 0));
          layoutSessionHead();
        }
      } else if (action === 'revert') {
        const turn = argument || await this.chooseRevert(view, uid);
        if (!turn || !current()) return true;
        if (argument && !await appConfirm(`回退对话到此轮之前？\n\n将移除 ${turn} 及之后的对话，仅修改会话历史，不回滚工作区文件。`, {ok:'回退'})) return true;
        if (!current()) return true;
        operation.timelineUid = uid;
        await view.request('api/codex-sdk/revert', {uid, before_turn_id:turn});
        if (current()) {
          if (input?.value === text) {
            input.value = ''; view.fitInput(); CodexSdkCompose.input(view);
          }
          if (await this.finishRevert(view, generation, operation))
            view.setNotice('对话已回退；工作区文件保持原样。');
        }
        succeeded = true;
      } else if (action === 'compact') {
        if (argument) throw new Error('用法：/compact（无需参数）');
        await view.request('api/codex-sdk/compact', {uid});
        if (current()) view.paintStatus();
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
      renameAudit('finished',{outcome:'failed'});
      if (current()) view.setNotice(`会话操作失败：${error.message}`, 'blocked');
    } finally {
      if (this.pending === operation) this.pending = false;
      if (current()) this.paint(view);
      if (operation.timelineUid) void followSelectedTimeline(false);
    }
    return true;
  },

  async openFork(view, response) {
    const sourceUid = view.uid, sourceAgent = view.agent;
    if (response.session) T.sdkSessions = [...T.sdkSessions.filter(row => row.uid !== response.uid), response.session];
    SdkView.selectMode(response.uid, null, 'sdk');
    // Native forks are normally persisted immediately, so hydrate their
    // inventory before opening instead of treating them as empty starts.
    if (!response.session?.sdk_pending || sourceUid.startsWith('kiro:')) await loadSessions(true, true);
    // An explicit fork keeps both branches available. Reuse the durable
    // parent visibility preference; child views must not reveal their owner.
    if (!sourceAgent && S.sessions.some(row => row.uid === sourceUid && row.fork_parent))
      await setForkParentVisibility([sourceUid], true);
    await loadSessions(true, true);
    await openSession(response.uid);
  },

  decorate(view, node, message) {
    if (message?.role !== 'user' || !message.turn_id) return;
    SdkUI.decorateUserBubble(view, node, [
      ['回退', () => this.messageAction(view, message, 'revert')],
      ['分支', () => this.messageAction(view, message, 'fork')],
    ]);
  },

  async restoreInput(view, text) {
    const generation = view.generation;
    await CodexSdkCompose.states.get(view)?.ready;
    if (!view.current(generation)) return;
    const input = view.input;
    if (!input) return;
    if (input.value.trim() && input.value !== text) input.value += '\n\n' + text;
    else input.value = text;
    view.fitInput(); CodexSdkCompose.input(view); input.focus();
    if (CodexSdkCompose.states.get(view)?.draft) await persistComposerDraft(view.uid);
  },

  async finishRevert(view, generation, operation) {
    const uid = view.uid;
    const current = () => view.current(generation) && view.uid === uid;
    if (!view.agent) {
      // A native revert can rotate the rollout without changing thread ID.
      // Resolve that identity now, before restoring an editable message, rather
      // than leaving a second navigation to the next inventory poll.
      const loaded = await loadSessions(true, true);
      if (!current()) return false;
      if (!loaded) throw new Error('对话已回退，但刷新会话列表失败，请重新打开会话。');
      const target = followContinuedSession(uid);
      if (target !== uid) {
        SdkView.selectMode(target, null, 'sdk');
        await followSelectedTimeline(false, operation);
        return view.uid === target && S.sel === target && !!view.pane;
      }
    }
    view.turn = null; view.items.clear(); view.nodes.clear(); view.live.replaceChildren();
    await view.refreshHistory(generation, true);
    return current();
  },

  async messageAction(view, message, action) {
    if (this.pending) return;
    const generation = view.generation, uid = view.uid, operation = {};
    const current = () => view.current(generation) && view.uid === uid;
    this.pending = operation; this.paint(view);
    try {
      if (action === 'revert') {
        if (!await appConfirm('回退到这条消息之前？该条及之后的对话会被移除，消息内容将放回输入框。工作区文件保持原样。', {ok:'回退'})) return;
        if (!current()) return;
        operation.timelineUid = uid;
        await view.request('api/codex-sdk/revert', {uid, before_turn_id:message.turn_id});
        if (!current()) return;
        if (await this.finishRevert(view, generation, operation))
          await this.restoreInput(view, String(message.text || ''));
      } else {
        const response = await view.request('api/codex-sdk/fork', {uid, last_turn_id:message.turn_id, before_turn_id:message.turn_id});
        if (!current()) return;
        await this.openFork(view, response);
        if (view.uid === response.uid) await this.restoreInput(view, String(message.text || ''));
      }
    } catch (error) { if (current()) view.setNotice(`消息操作失败：${error.message}`, 'blocked', true); }
    finally {
      if (this.pending === operation) this.pending = false;
      this.paint(view);
      if (operation.timelineUid) void followSelectedTimeline(false);
    }
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
