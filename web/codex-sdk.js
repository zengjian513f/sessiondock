'use strict';

// Codex SDK 视图（docs/codex-sdk.md）：与“对话/终端”并列的第三种会话视图。
// 已落盘的历史仍由普通读模型给出，气泡与消息模式完全相同；进行中的回合、
// 审批和提问来自节点与共享 Codex app-server 的连接（/api/codex-sdk/*）。
// 这里只负责呈现与交互，不解析终端画面，也不另存历史。
const CodexSdkView = {
  uid: '', pane: null, es: null, generation: 0,
  box: null, history: null, live: null, notice: null, input: null,
  turn: null, items: new Map(), nodes: new Map(), requests: new Map(),
  summary: null, models: [], historyTimer: 0, snapshotReady: null,

  available(uid, agent) {
    return SessionDockCapabilities.config.codex_sdk === true && !agent
      && String(uid || '').startsWith('codex:') && !String(uid).startsWith('tmux:');
  },

  unavailableReason(uid, agent) {
    if (SessionDockCapabilities.config.codex_sdk !== true) return '此服务未启用 Codex SDK 模式。';
    if (agent) return '请切换到主会话使用 SDK 模式。';
    if (!String(uid || '').startsWith('codex:')) return 'SDK 模式只支持 Codex 会话。';
    return '';
  },

  open(uid, pane) {
    this.close();
    this.uid = uid; this.pane = pane;
    const generation = ++this.generation;
    pane.setAttribute('aria-label', 'Codex SDK');
    pane.classList.add('sdk-pane');
    pane.innerHTML = `<div class="sdk-view">
      <div class="msgs sdk-msgs" aria-live="polite"><div class="sdk-history"></div><div class="sdk-live"></div></div>
      <div id="sdk-composer">
        <div class="sdk-status" role="status" aria-live="polite"></div>
        <div class="sdk-options">
          <label class="new-effort sdk-model" title="模型"><span class="visually-hidden">模型</span><select id="sdk-model"><option value="">默认模型</option></select></label>
          <label class="new-effort sdk-effort" title="推理强度"><span class="visually-hidden">推理强度</span><select id="sdk-effort"><option value="">默认强度</option></select></label>
          <span class="sdk-usage"></span>
        </div>
        <div class="composer-row">
          <div class="composer-input-wrap"><textarea id="sdk-input" rows="1" enterkeyhint="enter" placeholder="通过 Codex SDK 发送"></textarea></div>
          <div class="cbtns">
            <button class="btn" id="sdk-stop" type="button" hidden title="中断当前回合">停止</button>
            <button class="btn go" id="sdk-send" type="button">发送</button>
          </div>
        </div>
      </div></div>`;
    this.box = pane.querySelector('.sdk-msgs');
    this.history = pane.querySelector('.sdk-history');
    this.live = pane.querySelector('.sdk-live');
    this.notice = pane.querySelector('.sdk-status');
    this.input = pane.querySelector('#sdk-input');
    this.input.oninput = () => this.fitInput();
    this.input.onkeydown = event => {
      if (event.key === 'Enter' && !event.shiftKey && !event.isComposing && !MOBILE.matches) {
        event.preventDefault();
        this.send();
      }
    };
    pane.querySelector('#sdk-send').onclick = () => this.send();
    pane.querySelector('#sdk-stop').onclick = () => this.interrupt();
    pane.querySelector('#sdk-model').onchange = () => this.paintEfforts();
    this.setNotice('正在连接 Codex app-server…');
    this.start(generation);
  },

  close() {
    this.generation++;
    this.es?.close();
    this.es = null;
    clearTimeout(this.historyTimer);
    this.items.clear(); this.nodes.clear(); this.requests.clear();
    this.turn = null; this.summary = null;
    this.pane = this.box = this.history = this.live = this.notice = this.input = null;
    this.uid = '';
  },

  current(generation) { return generation === this.generation && !!this.pane; },

  async request(path, body) {
    const response = await fetch(appUrl(path), {
      method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify(body),
    });
    let data = null;
    try { data = await response.json(); } catch { /* 下面按 HTTP 状态说明 */ }
    if (!response.ok || !data?.ok) throw new Error(data?.error || `请求失败（HTTP ${response.status}）`);
    return data;
  },

  async start(generation) {
    const uid = this.uid;
    try {
      await this.request('api/codex-sdk/open', {uid});
    } catch (error) {
      if (!this.current(generation)) return;
      this.setNotice(`无法打开 SDK 模式：${error.message}`, 'blocked', true);
      return;
    }
    if (!this.current(generation)) return;
    this.loadModels(generation);
    let ready;
    this.snapshotReady = new Promise(resolve => { ready = resolve; });
    const es = new EventSource(appUrl(`api/codex-sdk/events?${new URLSearchParams({uid})}`));
    this.es = es;
    es.addEventListener('sdk', event => {
      if (!this.current(generation)) return;
      let packet;
      try { packet = JSON.parse(event.data); } catch { return; }
      if (packet.type === 'snapshot') { this.applySnapshot(packet.raw); ready(); return; }
      this.applyEvent(packet.method, packet.raw || {});
    });
    es.onerror = () => {
      if (!this.current(generation)) return;
      if (es.readyState === EventSource.CLOSED) this.setNotice('SDK 事件流已断开，正在重新连接…', 'blocked');
    };
    this.refreshHistory(generation);
  },

  reconnect() {
    if (!this.pane) return;
    const uid = this.uid, pane = this.pane;
    this.open(uid, pane);
  },

  async loadModels(generation) {
    try {
      const response = await fetch(appUrl(`api/codex-sdk/models?${new URLSearchParams({uid: this.uid})}`));
      const data = await response.json();
      if (!this.current(generation) || !data?.ok) return;
      this.models = (data.raw?.data || []).filter(model => !model.hidden);
      const select = this.pane.querySelector('#sdk-model');
      const chosen = select.value;
      select.replaceChildren(new Option('默认模型', ''));
      for (const model of this.models) select.append(new Option(model.displayName || model.model, model.model));
      select.value = chosen;
      this.paintEfforts();
    } catch { /* 模型列表只影响选择器，发送仍使用会话当前设置 */ }
  },

  paintEfforts() {
    if (!this.pane) return;
    const select = this.pane.querySelector('#sdk-effort');
    const chosen = select.value;
    const modelName = this.pane.querySelector('#sdk-model').value || this.summary?.settings?.model || '';
    const model = this.models.find(row => row.model === modelName) || this.models.find(row => row.isDefault);
    select.replaceChildren(new Option('默认强度', ''));
    for (const effort of model?.supportedReasoningEfforts || []) {
      const option = new Option(effort.reasoningEffort, effort.reasoningEffort);
      option.title = effort.description || '';
      select.append(option);
    }
    if ([...select.options].some(option => option.value === chosen)) select.value = chosen;
  },

  // ---------------------------------------------------------------- 历史
  async refreshHistory(generation) {
    try {
      const [{data}] = await Promise.all([fetchMessages(this.uid), this.snapshotReady]);
      if (!this.current(generation)) return;
      const stick = this.nearBottom();
      let messages = data?.messages || [];
      const userText = this.activeUserText();
      if (userText) {
        // 正在进行的回合已在 SDK 实时区显示；落盘历史截到它的用户消息之前。
        for (let i = messages.length - 1; i >= 0; i--) {
          if (messages[i].role === 'user' && String(messages[i].text || '').trim() === userText) {
            messages = messages.slice(0, i);
            break;
          }
        }
      }
      this.history.replaceChildren();
      appendMessages(this.history, messages);
      // 已经落盘的回合从实时区移除，避免同一回合出现两次。
      for (const [id, node] of this.nodes) {
        if (!this.items.has(id)) { node.remove(); this.nodes.delete(id); }
      }
      this.paintStatus();
      if (stick) this.scrollBottom();
    } catch (error) {
      if (this.current(generation)) this.setNotice(`读取历史失败：${error.message || error}`, 'blocked');
    }
  },

  scheduleHistory() {
    clearTimeout(this.historyTimer);
    const generation = this.generation;
    this.historyTimer = setTimeout(() => this.refreshHistory(generation), 600);
  },

  activeUserText() {
    if (!this.turn) return '';
    for (const item of this.items.values()) {
      if (item.type === 'userMessage') return userMessageText(item).trim();
    }
    return '';
  },

  // ---------------------------------------------------------------- 事件
  applySnapshot(raw) {
    this.summary = raw.summary || {};
    this.turn = raw.turn || null;
    this.items = new Map((raw.items || []).map(item => [item.id, item]));
    this.live.replaceChildren();
    this.nodes.clear();
    this.requests = new Map((raw.requests || []).map(request => [request.key, request]));
    for (const item of this.items.values()) this.paintItem(item);
    for (const request of this.requests.values()) this.paintRequest(request);
    this.paintEfforts();
    this.paintStatus();
    this.scrollBottom();
  },

  applyEvent(method, raw) {
    const stick = this.nearBottom();
    const item = raw.item;
    switch (method) {
      case 'bridge/reset':
        this.setNotice('与 Codex app-server 的连接已重置，正在重新连接…', 'blocked');
        this.es?.close();
        setTimeout(() => this.reconnect(), 800);
        return;
      case 'thread/resumed': this.summary = raw.summary || this.summary; break;
      case 'turn/started':
        this.turn = raw.turn; this.items.clear();
        if (this.summary) this.summary.last_error = null;
        break;
      case 'turn/completed':
        this.turn = null; this.items.clear();
        if (raw.turn?.status === 'failed' && raw.turn.error && this.summary) this.summary.last_error = raw.turn.error;
        this.scheduleHistory();
        break;
      case 'item/started': case 'item/completed':
        if (item?.id) { this.items.set(item.id, item); this.paintItem(item); }
        break;
      case 'item/agentMessage/delta': case 'item/plan/delta':
        this.appendDelta(raw.itemId, 'text', raw.delta); break;
      case 'item/commandExecution/outputDelta':
        this.appendDelta(raw.itemId, 'aggregatedOutput', raw.delta); break;
      case 'item/reasoning/summaryTextDelta': {
        const target = this.items.get(raw.itemId);
        if (target) {
          const parts = Array.isArray(target.summary) ? target.summary : (target.summary = []);
          const index = raw.summaryIndex || 0;
          while (parts.length <= index) parts.push('');
          parts[index] += raw.delta || '';
          this.paintItem(target);
        }
        break;
      }
      case 'serverRequest/resolved': {
        const key = JSON.stringify(raw.requestId);
        this.requests.delete(key);
        this.live.querySelector(`[data-sdk-request="${CSS.escape(key)}"]`)?.remove();
        break;
      }
      case 'thread/status/changed': if (this.summary) this.summary.status = raw.status; break;
      case 'thread/settings/updated': if (this.summary) this.summary.settings = raw.threadSettings; this.paintEfforts(); break;
      case 'thread/tokenUsage/updated': if (this.summary) this.summary.usage = raw.tokenUsage; break;
      case 'error': if (this.summary) this.summary.last_error = raw.error; break;
      default:
        if (SDK_REQUEST_METHODS.has(method)) {
          const request = {key: raw.key, method, params: raw.params || {}};
          this.requests.set(request.key, request);
          this.paintRequest(request);
        }
    }
    this.paintStatus();
    if (stick) this.scrollBottom();
  },

  appendDelta(id, field, delta) {
    const target = this.items.get(id);
    if (!target) return;
    target[field] = (target[field] || '') + (delta || '');
    this.paintItem(target);
  },

  // ---------------------------------------------------------------- 渲染
  paintItem(item) {
    const message = itemMessage(item);
    const old = this.nodes.get(item.id);
    if (!message) { old?.remove(); this.nodes.delete(item.id); return; }
    const node = msgNode(message);
    node.dataset.sdkItem = item.id;
    if (item.status === 'inProgress' && TOOL_ROLES.has(message.role)) {
      const status = document.createElement('div');
      status.className = 'tool-status';
      status.textContent = '运行中…';
      node.querySelector('.tool-entry')?.append(status);
    }
    if (old) old.replaceWith(node);
    else {
      const firstRequest = this.live.querySelector('[data-sdk-request]');
      this.live.insertBefore(node, firstRequest);
    }
    this.nodes.set(item.id, node);
  },

  paintRequest(request) {
    const old = this.live.querySelector(`[data-sdk-request="${CSS.escape(request.key)}"]`);
    const card = requestCard(request, result => this.answer(request, result, card));
    card.dataset.sdkRequest = request.key;
    if (old) old.replaceWith(card); else this.live.append(card);
  },

  paintStatus() {
    if (!this.pane) return;
    const active = !!this.turn;
    this.pane.querySelector('#sdk-stop').hidden = !active;
    const send = this.pane.querySelector('#sdk-send');
    send.textContent = active ? '追加' : '发送';
    send.title = active ? '追加到进行中的回合（turn/steer）' : '开始新回合（turn/start）';
    const usage = this.summary?.usage;
    const window = usage?.modelContextWindow, used = usage?.last?.totalTokens ?? usage?.total?.totalTokens;
    this.pane.querySelector('.sdk-usage').textContent = window && used
      ? `上下文 ${Math.min(100, Math.round(used / window * 100))}%` : '';
    const status = this.summary?.status || {};
    const parts = [];
    if (S.live.has(this.uid)) {
      parts.push('该会话的 CLI 进程仍在运行。SessionDock 启动的终端使用独立运行时，与 SDK 同时发送会形成两份上下文；建议先在终端退出 Codex。');
    }
    if (this.summary?.last_error?.message) parts.push(`错误：${this.summary.last_error.message}`);
    if (status.type === 'systemError') parts.push('Codex 报告系统错误。');
    else if (status.activeFlags?.includes('waitingOnApproval')) parts.push('等待批准。');
    else if (status.activeFlags?.includes('waitingOnUserInput')) parts.push('等待回答。');
    else if (active) parts.push('Codex 正在处理…');
    this.setNotice(parts.join(' '), this.summary?.last_error || status.type === 'systemError' ? 'blocked' : '');
  },

  setNotice(text, kind = '', sticky = false) {
    if (!this.notice) return;
    this.notice.textContent = text;
    this.notice.hidden = !text;
    this.notice.className = 'sdk-status' + (kind ? ` ${kind}` : '');
    this.notice.dataset.sticky = sticky ? '1' : '';
  },

  nearBottom() {
    const box = this.box;
    return !box || box.scrollHeight - box.scrollTop - box.clientHeight < 80;
  },

  scrollBottom() {
    if (this.box) this.box.scrollTop = this.box.scrollHeight;
  },

  fitInput() {
    const input = this.input;
    if (!input) return;
    input.style.height = 'auto';
    input.style.height = Math.min(input.scrollHeight, 240) + 'px';
    input.style.overflowY = input.scrollHeight > 240 ? 'auto' : 'hidden';
  },

  // ---------------------------------------------------------------- 操作
  async send() {
    const input = this.input;
    const text = input?.value || '';
    if (!text.trim()) return;
    if (S.live.has(this.uid) && !this.turn && !this.confirmedLive) {
      const ok = await appConfirm('该会话的 CLI 进程仍在运行，SDK 将在另一个运行时继续同一会话。仍然发送？',
        {ok: '仍然发送'});
      if (!ok) return;
      this.confirmedLive = this.uid;
    }
    const button = this.pane.querySelector('#sdk-send');
    button.disabled = true;
    button.setAttribute('aria-busy', 'true');
    try {
      await this.request('api/codex-sdk/turn', {
        uid: this.uid, text,
        model: this.pane.querySelector('#sdk-model').value || null,
        effort: this.pane.querySelector('#sdk-effort').value || null,
      });
      if (this.input === input) { input.value = ''; this.fitInput(); }
    } catch (error) {
      this.setNotice(`发送失败：${error.message}`, 'blocked');
    } finally {
      button.disabled = false;
      button.removeAttribute('aria-busy');
    }
  },

  async interrupt() {
    try {
      await this.request('api/codex-sdk/interrupt', {uid: this.uid});
    } catch (error) {
      this.setNotice(`中断失败：${error.message}`, 'blocked');
    }
  },

  async answer(request, result, card) {
    card.querySelectorAll('button, input, textarea').forEach(control => { control.disabled = true; });
    try {
      await this.request('api/codex-sdk/respond', {uid: this.uid, key: request.key, result});
      card.classList.add('sdk-answered');
    } catch (error) {
      card.querySelectorAll('button, input, textarea').forEach(control => { control.disabled = false; });
      this.setNotice(`回答失败：${error.message}`, 'blocked');
    }
  },
};

const SDK_REQUEST_METHODS = new Set([
  'item/commandExecution/requestApproval', 'item/fileChange/requestApproval',
  'item/tool/requestUserInput', 'item/permissions/requestApproval', 'mcpServer/elicitation/request',
]);

function userMessageText(item) {
  return (item.content || []).map(part => {
    if (part.type === 'text') return part.text || '';
    if (part.type === 'image') return `[图片] ${part.url || ''}`;
    if (part.type === 'localImage') return `[图片] ${part.path || ''}`;
    if (part.type === 'skill' || part.type === 'mention') return '';
    return '';
  }).filter(Boolean).join('\n');
}

const SDK_JSON = value => typeof value === 'string' ? value : JSON.stringify(value ?? null, null, 2);

/** app-server ThreadItem → 消息模式的消息对象，由同一个 msgNode 渲染。 */
function itemMessage(item) {
  const done = ['completed', 'failed', 'declined'].includes(item.status);
  const failed = item.status === 'failed' || item.status === 'declined';
  switch (item.type) {
    case 'userMessage': return {role: 'user', text: userMessageText(item)};
    case 'agentMessage': return item.text ? {role: 'assistant', text: item.text, phase: item.phase} : null;
    case 'plan': return item.text ? {role: 'assistant', text: item.text} : null;
    case 'reasoning': {
      const text = (item.summary || []).filter(Boolean).join('\n\n');
      return text ? {role: 'thinking', text} : null;
    }
    case 'commandExecution': {
      const command = item.commandActions?.[0]?.command || item.command || '';
      const output = item.aggregatedOutput || '';
      return {role: 'tool', name: 'shell', summary: `$ ${command}`, text: item.command || command,
        result: done || output ? {role: 'tool_result', text: output, exit_code: item.exitCode ?? undefined,
          duration_s: item.durationMs != null ? item.durationMs / 1000 : undefined,
          error: failed || (item.exitCode != null && item.exitCode !== 0)} : null};
    }
    case 'fileChange': {
      const changes = item.changes || [];
      const kind = change => typeof change.kind === 'string' ? change.kind : change.kind?.type || '';
      return {role: 'tool', name: 'apply_patch', summary: `修改 ${changes.length} 个文件`,
        text: changes.map(change => `${kind(change)} ${change.path}`.trim()).join('\n'),
        result: done ? {role: 'tool_result', text: changes.map(change => change.diff || '').join('\n'),
          error: failed} : null};
    }
    case 'mcpToolCall':
      return {role: 'tool', name: `${item.server}.${item.tool}`, summary: `${item.server} · ${item.tool}`,
        text: SDK_JSON(item.arguments),
        result: done ? {role: 'tool_result', text: item.error ? SDK_JSON(item.error) : SDK_JSON(item.result),
          error: !!item.error || failed} : null};
    case 'dynamicToolCall':
      return {role: 'tool', name: item.tool, summary: item.tool, text: SDK_JSON(item.arguments),
        result: done ? {role: 'tool_result', text: SDK_JSON(item.contentItems), error: item.success === false || failed} : null};
    case 'collabToolCall':
      return {role: 'tool', name: item.tool, summary: `${item.tool}${item.prompt ? ' · ' + item.prompt : ''}`,
        text: SDK_JSON(item), result: done ? {role: 'tool_result', text: SDK_JSON(item.agentStatus || ''), error: failed} : null};
    case 'webSearch':
      return {role: 'tool', name: 'web_search', summary: `搜索 ${item.query || item.action?.query || item.action?.url || ''}`,
        text: SDK_JSON(item.action || item.query)};
    case 'imageView': return {role: 'tool', name: 'view_image', summary: `查看图片 ${item.path || ''}`, text: item.path || ''};
    case 'enteredReviewMode': return {role: 'assistant', text: `开始审查：${item.review || ''}`};
    case 'exitedReviewMode': return item.review ? {role: 'assistant', text: item.review} : null;
    case 'contextCompaction': return {role: 'tool', name: 'compact', summary: '上下文已压缩', text: ''};
    case 'functionCallOutput':
      return {role: 'tool_result', name: item.name, text: SDK_JSON(item.output)};
    default: return {role: 'tool', name: item.type, summary: item.type, text: SDK_JSON(item)};
  }
}

const SDK_DECISIONS = {
  accept: '批准', acceptForSession: '本会话内批准', decline: '拒绝', cancel: '拒绝并中断',
};

function sdkDecisionLabel(decision) {
  if (typeof decision === 'string') return SDK_DECISIONS[decision] || decision;
  if (decision?.acceptWithExecpolicyAmendment) return '批准并记住此命令';
  const network = decision?.applyNetworkPolicyAmendment?.network_policy_amendment;
  if (network) return `${network.action === 'deny' ? '始终拒绝' : '始终允许'} ${network.host}`;
  return JSON.stringify(decision);
}

/** 服务端请求的题卡：审批、提问、权限、MCP 表单。按钮回传原生结构。 */
function requestCard(request, submit) {
  const params = request.params || {};
  const card = document.createElement('div');
  card.className = 'msg sdk-request';
  card.dataset.role = 'question';
  const body = document.createElement('div');
  body.className = 'mb';
  card.append(body);
  const line = (text, cls = '') => {
    if (!text) return;
    const p = document.createElement('p');
    if (cls) p.className = cls;
    p.textContent = text;
    body.append(p);
  };
  const pre = text => {
    if (!text) return;
    const block = document.createElement('pre');
    block.textContent = text;
    body.append(block);
  };
  const actions = document.createElement('div');
  actions.className = 'sdk-request-actions';
  const button = (label, result, go = false) => {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'btn' + (go ? ' go' : '');
    b.textContent = label;
    b.onclick = () => submit(typeof result === 'function' ? result() : result);
    actions.append(b);
    return b;
  };
  switch (request.method) {
    case 'item/commandExecution/requestApproval': {
      const network = params.networkApprovalContext;
      line(network ? `需要批准网络访问：${network.protocol || ''} ${network.host || ''}` : '需要批准执行命令', 'sdk-request-title');
      if (!network) pre(params.command);
      line(params.cwd ? `目录：${params.cwd}` : '');
      line(params.reason);
      const decisions = params.availableDecisions || ['accept', 'acceptForSession', 'decline', 'cancel'];
      decisions.forEach((decision, index) => button(sdkDecisionLabel(decision), {decision}, index === 0));
      break;
    }
    case 'item/fileChange/requestApproval':
      line('需要批准文件修改', 'sdk-request-title');
      line(params.reason);
      line(params.grantRoot ? `授权目录：${params.grantRoot}` : '');
      ['accept', 'acceptForSession', 'decline', 'cancel']
        .forEach((decision, index) => button(SDK_DECISIONS[decision], {decision}, index === 0));
      break;
    case 'item/permissions/requestApproval':
      line('需要授予额外权限', 'sdk-request-title');
      line(params.reason);
      pre(SDK_JSON(params.permissions));
      button('授予（本回合）', {permissions: params.permissions || {}, scope: 'turn'}, true);
      button('授予（本会话）', {permissions: params.permissions || {}, scope: 'session'});
      button('拒绝', {permissions: {}, scope: 'turn'});
      break;
    case 'item/tool/requestUserInput': {
      line('Codex 需要你回答', 'sdk-request-title');
      const readers = [];
      for (const question of params.questions || []) {
        const group = document.createElement('fieldset');
        group.className = 'sdk-question';
        const legend = document.createElement('legend');
        legend.textContent = [question.header, question.question].filter(Boolean).join(' · ');
        group.append(legend);
        const name = `sdk-q-${request.key}-${question.id}`;
        for (const option of question.options || []) {
          const label = document.createElement('label');
          const radio = document.createElement('input');
          radio.type = 'radio'; radio.name = name; radio.value = option.label;
          label.append(radio, ` ${option.label}`);
          if (option.description) label.title = option.description;
          group.append(label);
        }
        let other = null;
        if (question.isOther || !(question.options || []).length) {
          other = document.createElement('input');
          other.type = question.isSecret ? 'password' : 'text';
          other.placeholder = (question.options || []).length ? '其他答案' : '输入答案';
          group.append(other);
        }
        body.append(group);
        readers.push(() => {
          const picked = group.querySelector('input[type=radio]:checked')?.value;
          const typed = other?.value.trim();
          return [question.id, {answers: typed ? [typed] : picked ? [picked] : []}];
        });
      }
      button('提交', () => ({answers: Object.fromEntries(readers.map(read => read()))}), true);
      break;
    }
    case 'mcpServer/elicitation/request': {
      line(`${params.serverName || 'MCP'} 请求确认`, 'sdk-request-title');
      line(params.message);
      if (params.url) {
        const link = document.createElement('a');
        link.href = params.url; link.target = '_blank'; link.rel = 'noopener';
        link.textContent = params.url;
        body.append(link);
        button('已完成', {action: 'accept', content: null}, true);
      } else {
        pre(SDK_JSON(params.requestedSchema));
        const input = document.createElement('textarea');
        input.className = 'sdk-json';
        input.placeholder = '按上面的结构填写 JSON';
        input.value = '{}';
        body.append(input);
        button('提交', () => {
          try { return {action: 'accept', content: JSON.parse(input.value || '{}')}; }
          catch { return {action: 'accept', content: {}}; }
        }, true);
      }
      button('拒绝', {action: 'decline', content: null});
      button('取消', {action: 'cancel', content: null});
      break;
    }
    default:
      line(request.method);
      pre(SDK_JSON(params));
  }
  card.append(actions);
  return card;
}
