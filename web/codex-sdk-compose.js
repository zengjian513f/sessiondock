'use strict';

// SDK and the transitional conversation view edit the same server-owned draft.
// Reuse its CAS saves, private staging, resumable uploads and attachment cards.
const CodexSdkCompose = {
  states: new WeakMap(),

  mount(view) {
    const state = {uid: view.uid, input: view.input, pane: view.pane, sending: false};
    this.states.set(view, state);
    state.input.disabled = true;
    state.ready = new Promise(resolve => {
      const start = () => {
        if (this.states.get(view) !== state) { resolve(); return; }
        if (view.agent || typeof composerDraft !== 'function' || !conversationSendEnabled()
            || (HUB_MODE && Nodes.capabilities[nodeOf(state.uid)]?.conversation_send !== true)) {
          state.input.disabled = false;
          resolve(); return;
        }
        state.draft = composerDraft(state.uid);
        this.controls(view, state);
        state.follow = () => this.follow(view);
        window.addEventListener('focus', state.follow);
        window.addEventListener('online', state.follow);
        document.addEventListener('visibilitychange', state.follow);
        state.timer = setInterval(() => {
          if (!document.hidden) this.follow(view);
        }, 1500);
        hydrateComposerDraft(state.uid).then(() => {
          if (this.states.get(view) === state) this.paint(view, state);
          resolve();
        });
      };
      // app.js can restore an SDK page before the later deferred term.js has
      // initialized the shared draft store. Never touch its lexical state early.
      if (typeof composerDraft !== 'function' && document.readyState !== 'complete')
        document.addEventListener('DOMContentLoaded', start, {once: true});
      else start();
    });
  },

  close(view) {
    const state = this.states.get(view);
    if (!state) return;
    clearInterval(state.timer);
    window.removeEventListener('focus', state.follow);
    window.removeEventListener('online', state.follow);
    document.removeEventListener('visibilitychange', state.follow);
    state.unbind?.abort();
    this.states.delete(view);
  },

  controls(view, state) {
    const composer = state.pane.querySelector('#sdk-composer');
    state.cards = document.createElement('div');
    state.cards.id = 'sdk-compose-items';
    state.cards.setAttribute('aria-live', 'polite');
    composer.querySelector('.composer-row').before(state.cards);
    const picker = document.createElement('div');
    picker.className = 'attach-picker';
    picker.innerHTML = `<button class="btn" id="sdk-add" type="button" title="添加附件或引用" aria-label="添加附件或引用" aria-expanded="false">
      <svg class="ui-icon" aria-hidden="true"><use href="#i-plus"/></svg></button>
      <div class="attach-menu hidden" id="sdk-attach-menu" role="menu">
        <button type="button" data-attach="image" role="menuitem"><span>▧</span>图片</button>
        <button type="button" data-attach="file" role="menuitem"><span>⌑</span>文件</button>
        <button type="button" data-attach="quote" role="menuitem"><span>❝</span>引用文字</button>
      </div><input id="sdk-file" type="file" multiple hidden>`;
    composer.querySelector('.composer-row').prepend(picker);
    state.add = picker.querySelector('#sdk-add');
    const menu = picker.querySelector('#sdk-attach-menu'), file = picker.querySelector('#sdk-file');
    const close = () => {
      menu.classList.add('hidden'); state.add.classList.remove('on');
      state.add.setAttribute('aria-expanded', 'false');
    };
    state.add.onclick = () => {
      const open = menu.classList.toggle('hidden') === false;
      state.add.classList.toggle('on', open);
      state.add.setAttribute('aria-expanded', String(open));
    };
    for (const button of menu.querySelectorAll('button')) button.onclick = () => {
      close();
      if (button.dataset.attach === 'quote') {
        state.draft.quotes.push({id: crypto.randomUUID(), text: ''});
        this.save(view, state);
        state.cards.querySelector('.draft-quote:last-of-type textarea')?.focus();
      } else chooseAttachmentFiles(button.dataset.attach, file, files => this.add(view, state, files));
    };
    file.onchange = () => { this.add(view, state, [...file.files]); file.value = ''; };
    state.unbind = new AbortController();
    document.addEventListener('click', event => {
      if (!picker.contains(event.target)) close();
    }, {signal: state.unbind.signal});
    state.input.addEventListener('paste', event => pasteAttachmentFiles(event,
      files => this.add(view, state, files)), {signal: state.unbind.signal});
    bindFileDrop(composer, files => this.add(view, state, files));
  },

  // Suggestions are local text templates, never shell evaluation or execution.
  shellCommands: ['cat', 'cd', 'echo', 'find', 'git', 'head', 'ls', 'mkdir', 'pwd', 'rg', 'tail'],
  shellGitCommands: ['add', 'branch', 'checkout', 'commit', 'diff', 'fetch', 'log', 'pull', 'push', 'restore', 'show', 'stash', 'status', 'switch'],

  closeCompletion(view) {
    view.pane?.querySelector('#sdk-shell-completion')?.remove();
    view.input?.setAttribute('aria-expanded', 'false');
    view.input?.removeAttribute('aria-activedescendant');
    view.input?.removeAttribute('aria-controls');
    view.shellCompletion = null;
  },

  shellKey(view, event) {
    if (event.isComposing || event.ctrlKey || event.metaKey || event.altKey
        || ['Shift', 'Control', 'Meta', 'Alt'].includes(event.key)) return false;
    const input = view.input;
    let completion = view.shellCompletion;
    if (completion && (completion.text !== input.value || completion.cursor !== input.selectionStart
        || input.selectionStart !== input.selectionEnd)) {
      this.closeCompletion(view); completion = null;
    }
    const accept = value => {
      input.setRangeText(value + ' ', completion.start, completion.cursor, 'end');
      this.closeCompletion(view);
      input.focus(); view.fitInput(); this.input(view);
    };
    if (completion) {
      if (event.key === 'Escape') { event.preventDefault(); this.closeCompletion(view); return true; }
      if (event.key === 'Enter') { event.preventDefault(); accept(completion.rows[completion.active]); return true; }
      if (['Tab', 'ArrowDown', 'ArrowUp'].includes(event.key)) {
        event.preventDefault();
        const step = event.key === 'ArrowUp' || event.shiftKey ? -1 : 1;
        completion.active = (completion.active + step + completion.rows.length) % completion.rows.length;
        completion.buttons.forEach((button, index) => {
          button.classList.toggle('selected', index === completion.active);
          button.setAttribute('aria-selected', String(index === completion.active));
        });
        const button = completion.buttons[completion.active];
        input.setAttribute('aria-activedescendant', button.id);
        button.scrollIntoView({block: 'nearest'});
        return true;
      }
      this.closeCompletion(view);
    }
    if (event.key !== 'Tab' || event.shiftKey || input.selectionStart !== input.selectionEnd) return false;
    if (/[^\s]/.test(input.value.charAt(input.selectionStart))) return false;
    const before = input.value.slice(0, input.selectionStart);
    const match = before.match(/^\s*(?:!\s*|\/shell\s+)(git\s+)?([a-z]*)$/);
    if (!match) return false;
    const prefix = match[2];
    const rows = (match[1] ? this.shellGitCommands : this.shellCommands).filter(value => value.startsWith(prefix));
    if (!rows.length) return false;
    event.preventDefault();
    completion = {text: input.value, cursor: input.selectionStart,
      start: input.selectionStart - prefix.length, rows, active: 0, buttons: []};
    if (rows.length === 1) { accept(rows[0]); return true; }
    view.shellCompletion = completion;
    const box = el('div', 'input-history');
    box.id = 'sdk-shell-completion';
    box.append(el('div', 'input-history-head', '常用命令 · Tab / ↑↓ 选择 · Enter 填入 · Esc 关闭'));
    const list = el('div', 'input-history-list');
    list.setAttribute('role', 'listbox');
    list.setAttribute('aria-label', '常用命令建议');
    list.id = 'sdk-shell-options';
    for (const [index, value] of rows.entries()) {
      const button = el('button', 'input-history-item' + (index === 0 ? ' selected' : ''), value);
      button.type = 'button'; button.tabIndex = -1;
      button.id = `sdk-shell-option-${index}`;
      button.setAttribute('role', 'option');
      button.setAttribute('aria-selected', String(index === 0));
      button.onmousedown = event => event.preventDefault();
      button.onclick = () => accept(value);
      completion.buttons.push(button); list.append(button);
    }
    box.append(list); input.parentElement.append(box);
    input.setAttribute('aria-controls', list.id);
    input.setAttribute('aria-expanded', 'true');
    input.setAttribute('aria-activedescendant', completion.buttons[0].id);
    return true;
  },

  input(view) {
    this.closeCompletion(view);
    const state = this.states.get(view);
    if (!state?.draft) return;
    state.draft.text = state.input.value;
    this.save(view, state);
  },

  save(view, state) {
    const saving = persistComposerDraft(state.uid);
    this.paint(view, state);
    saving.then(() => this.paint(view, state));
    return saving;
  },

  add(view, state, files) {
    if (state.draft.loading || this.states.get(view) !== state) return;
    const before = new Set(state.draft.attachments);
    addDraftFiles(state.draft, files);
    this.save(view, state);
    for (const attachment of state.draft.attachments) {
      if (!before.has(attachment)) stageComposerAttachment(attachment, state.uid,
        {render: () => this.paint(view, state)});
    }
  },

  insert(view, state, number) {
    const input = state.input;
    input.setRangeText(`[附件${number}]`, input.selectionStart, input.selectionEnd, 'end');
    input.focus(); this.input(view); view.fitInput();
  },

  remove(view, state, id) {
    const attachment = state.draft.attachments.find(item => item.id === id);
    if (!attachment) return;
    attachment.cancelUpload?.();
    if (attachment.preview) URL.revokeObjectURL(attachment.preview);
    state.draft.attachments = state.draft.attachments.filter(item => item.id !== id);
    discardStagedAttachment(attachment, this.save(view, state));
  },

  paint(view, state = this.states.get(view)) {
    if (!state?.draft || this.states.get(view) !== state) return;
    const draft = state.draft;
    const focused = document.activeElement;
    const focusedQuote = focused?.closest?.('.draft-quote')?.dataset.quoteId;
    const selection = focusedQuote ? [focused.selectionStart, focused.selectionEnd] : null;
    if (state.input.value !== draft.text) { this.closeCompletion(view); state.input.value = draft.text; view.fitInput(); }
    state.input.disabled = !!draft.loading;
    state.add.disabled = !!draft.loading;
    renderAttachmentCards(state.cards, draft.attachments, {
      uid: state.uid, render: () => this.paint(view, state),
      onInsert: number => this.insert(view, state, number),
      onRemove: id => this.remove(view, state, id),
      onRetry: attachment => stageComposerAttachment(attachment, state.uid,
        {render: () => this.paint(view, state)}),
    });
    for (const quote of draft.quotes) {
      const card = el('div', 'draft-card draft-quote');
      card.dataset.quoteId = quote.id;
      const mark = el('span', '', '❝'), input = document.createElement('textarea');
      input.value = quote.text; input.placeholder = '粘贴或输入要引用的文字';
      input.setAttribute('aria-label', '引用文字');
      // Save without rebuilding the focused textarea on every keystroke.
      input.oninput = () => { quote.text = input.value; persistComposerDraft(state.uid); };
      const remove = el('button', 'draft-remove', '×');
      remove.type = 'button'; remove.title = remove.ariaLabel = '移除引用';
      remove.onclick = () => {
        draft.quotes = draft.quotes.filter(item => item.id !== quote.id);
        this.save(view, state);
      };
      card.append(mark, input, remove); state.cards.append(card);
      if (quote.id === focusedQuote) {
        input.focus({preventScroll: true}); input.setSelectionRange(...selection);
      }
    }
    renderSavedComposerInputs(state.cards, draft);
  },

  async follow(view) {
    const state = this.states.get(view);
    if (!state?.draft || state.sending || document.hidden) return;
    await hydrateComposerDraft(state.uid, true);
    // The shared follow routine retains local File objects and refuses to
    // replace local edits while their CAS save is still pending.
    await followServerDraft(state.uid);
    if (!state.cards.contains(document.activeElement)) this.paint(view, state);
  },

  async collect(view) {
    const state = this.states.get(view);
    await state?.ready;
    if (!state || this.states.get(view) !== state) throw new Error('会话已切换，输入仍保留在原会话草稿中');
    const uid = state.uid, draft = state.draft;
    if (!draft) return state.input.value.trim() ? {uid, text: state.input.value, attachments: [], quotes: [], state} : null;
    if (draft.loading || draft.loadFailed) throw new Error(draft.storageError || '草稿尚未读取完成');
    const draftText = state.input.value, files = [...draft.attachments];
    const quotes = draft.quotes.map(item => ({id: item.id, text: item.text})).filter(item => item.text.trim());
    if (!draftText.trim() && !files.length && !quotes.length) return null;
    state.sending = true;
    try {
      const attachments = [];
      for (const attachment of files) {
        if (attachment.staging) await attachment.staging;
        if (attachment.uploadCancelled) throw new Error('附件上传已取消；输入已保留。');
        const uploaded = await uploadComposerAttachment(attachment, uid, null,
          {render: () => this.paint(view, state)});
        attachments.push({upload_id: uploaded.upload_id, number: attachment.number});
      }
      if (!await persistComposerDraft(uid)) throw new Error(draft.storageError || '草稿尚未保存');
      return {uid, text: draftText, attachments, quotes, draft_revision: draft.revision,
        draftText, files, draft, state};
    } catch (error) { state.sending = false; throw error; }
  },

  async sent(view, payload) {
    if (!payload) return;
    const state = this.states.get(view);
    if (payload.draft) await consumeComposerSubmission(payload.uid, payload.draftText, payload.files, payload.quotes);
    else if (state?.uid === payload.uid && state.input.value === payload.text) state.input.value = '';
    if (state === payload.state) state.sending = false;
    if (state?.uid === payload.uid) { this.paint(view, state); view.fitInput(); }
  },

  async submission(payload, model, effort, agent) {
    const holder = payload.draft || payload.state;
    const key = JSON.stringify({uid: payload.uid, agent, text: payload.text,
      attachments: payload.attachments, quotes: payload.quotes, model, effort});
    if (holder.sdkSubmission?.key !== key) holder.sdkSubmission = {key, id: crypto.randomUUID()};
    if (payload.draft && !await persistComposerDraft(payload.uid))
      throw new Error(payload.draft.storageError || '草稿尚未保存');
    return holder.sdkSubmission.id;
  },

  failed(view) {
    const state = this.states.get(view);
    if (state) state.sending = false;
  },
};
