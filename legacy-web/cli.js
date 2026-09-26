'use strict';

/**
 * 前端 CLI 行为的公共基类。
 *
 * app.js 只保存和渲染乐观消息；某条原生记录是否能确认/结束排队、
 * 旧状态如何迁移、特殊键是否取消队列，都由具体 CLI 实现决定。
 */
class SessionDockCli {
  constructor(source, name, icon, color) {
    this.source = source;
    this.name = name;
    this.icon = icon;
    this.color = color;
  }

  migrateQueuedMessages(items, _fromVersion, _toVersion) {
    return Array.isArray(items) ? items : [];
  }

  createQueuedMessage(fields) {
    return { ...fields, state: 'queued' };
  }

  queueAction(message) {
    return ['user', 'command'].includes(message?.role)
      ? { type: 'remove', text: String(message.text || '') } : null;
  }

  normalizeQueuedText(value) {
    return String(value ?? '');
  }

  queuedTextMatches(pending, native) {
    return this.normalizeQueuedText(pending) === this.normalizeQueuedText(native);
  }

  settleQueuedMessage(item, _now, _hasNativeHistory) {
    return item;
  }

  queuedMessageLabel(item) {
    if (item?.state === 'failed') return '发送未确认';
    return item?.state === 'sending' ? '发送中' : '排队中';
  }

  questionAnswerKeys(_prompt, _optionIndex) {
    return null;
  }

  canAnswerQuestionForm(_prompt) {
    return false;
  }

  questionFormAnswerKeys(_prompt, _optionIndexes) {
    return null;
  }

  questionFormAnswerKeyGroups(_prompt, _optionIndexes) {
    return null;
  }

  questionCancelKeys() {
    return ['Escape'];
  }

  repeatedEscape(_now, _previousAt, _context = {}) {
    return { rewind: false, nextAt: -Infinity };
  }
}

class ClaudeCli extends SessionDockCli {
  constructor() {
    super('claude', 'Claude', 'i-claude', 'var(--claude)');
  }

  migrateQueuedMessages(items, fromVersion, _toVersion) {
    // v5 起正式 Claude 会话由服务端发送账本管理。旧浏览器副本既没有
    // request id 也没有交付凭据，保留只会再次制造“假失败/盲目重试”。
    if (fromVersion < 5) return [];
    return super.migrateQueuedMessages(items);
  }

  createQueuedMessage(fields) {
    return {
      ...fields,
      state: 'sending',
      // Claude 的 user/enqueue 记录实测会立即落盘。超时仍没有任何
      // 原生回执，只能说明 tmux 收到了按键，不能继续声称“排队中”。
      expiresAt: fields.created + 8000,
    };
  }

  normalizeQueuedText(value) {
    // Claude 的原生 user/command 和服务端账本都按编辑器提交语义去掉首尾空白。
    return String(value ?? '').trim();
  }

  queueAction(message) {
    const normal = super.queueAction(message);
    if (normal !== null) return normal;
    if (message?.role !== 'queue_operation') return null;
    if (message.operation === 'enqueue') {
      return { type: 'confirm', text: String(message.text || '') };
    }
    if (message.operation === 'remove') {
      return { type: 'remove', text: String(message.text || '') };
    }
    // Claude 2.1.226 在当前回合结束/中断后，为每条即将提升成正式 user
    // 消息的输入写一个无正文 dequeue；旧版本使用带正文的 popAll。
    if (message.operation === 'dequeue') return { type: 'promote-first' };
    if (message.operation === 'popAll') return { type: 'promote-all' };
    return null;
  }

  settleQueuedMessage(item, now, _hasNativeHistory) {
    if (item?.server) return item;
    if (item?.state !== 'sending' || !Number.isFinite(+item.expiresAt)
        || now < +item.expiresAt) return item;
    const settled = {
      ...item,
      state: 'failed',
      error: 'Claude 未在会话记录中确认接收',
    };
    delete settled.expiresAt;
    return settled;
  }

  queuedMessageLabel(item) {
    if (!item?.server) return super.queuedMessageLabel(item);
    if (item.state === 'restored') return '已中断，正文在终端草稿中';
    if (item.state === 'aborted') return '已中断';
    if (item.state === 'native_queued') return 'Claude 已排队';
    if (item.state === 'ambiguous' || item.state === 'injecting') return '状态待核对';
    if (item.state === 'persisted') return '等待提交';
    return '已送达终端，等待 Claude 确认';
  }

  // Claude Code 2.1.28x 的问题菜单：选项后追加 "Type something." 与
  // "Chat about this"。Up 在第一个选项处循环到 "Type something."，Down 夹在
  // "Chat about this"；光标停在文本行时数字键与 Left/Right 会被当成输入。
  // 先 Down 夹到底、再 Up×2 回到最后一个真实选项，此后的按键与原生终端或
  // 另一网页先前把光标移到哪里无关（真实 TUI 实测）。
  questionOptionRowKeys(optionCount) {
    return [...Array(optionCount + 2).fill('Down'), 'Up', 'Up'];
  }

  questionAnswerKeys(prompt, optionIndex) {
    const options = prompt?.questions?.[0]?.options || [];
    if (!options[optionIndex] || optionIndex >= 9) return null;
    // 在真实选项行上按数字即选中并提交。
    return [...this.questionOptionRowKeys(options.length), String(optionIndex + 1)];
  }

  canAnswerQuestionForm(prompt) {
    const questions = prompt?.questions;
    return Array.isArray(questions) && questions.length > 1
      && questions.every(q => !q?.multiple && q?.options?.length);
  }

  questionFormAnswerKeys(prompt, optionIndexes) {
    const groups = this.questionFormAnswerKeyGroups(prompt, optionIndexes);
    return groups?.flat() || null;
  }

  questionFormAnswerKeyGroups(prompt, optionIndexes) {
    const questions = prompt?.questions;
    if (!this.canAnswerQuestionForm(prompt) || !Array.isArray(optionIndexes)
        || optionIndexes.length !== questions.length) return null;
    const most = Math.max(...questions.map(q => q.options.length));
    // Left 只在选项行上切题，且在第一题夹住：每次先回到选项行再 Left，
    // 从任意一题或 Review 页都能回到第一题。
    const groups = questions.map(() => [...this.questionOptionRowKeys(most), 'Left']);
    for (let i = 0; i < questions.length; i++) {
      const optionIndex = optionIndexes[i];
      const options = questions[i].options;
      if (!Number.isInteger(optionIndex) || !options[optionIndex]) return null;
      // 选项菜单会循环，不能靠 Up 夹到顶：从夹住的 "Chat about this"
      // 往上数到目标项。Enter 选中后自动进入下一题，最后一题进入 Review。
      groups.push([...Array(options.length + 2).fill('Down'),
        ...Array(options.length + 1 - optionIndex).fill('Up'), 'Enter']);
    }
    // Review 页两项循环，光标可能留在 Cancel；数字 1 直接提交。
    groups.push(['1']);
    return groups;
  }

  repeatedEscape(now, previousAt, context = {}) {
    // 运行中 Esc 是中断，对话框中 Esc 是取消；两者都不是 rewind 的第一击。
    if (context.busy || context.empty === false) {
      return { rewind: false, nextAt: -Infinity };
    }
    const rewind = now - previousAt <= 650;
    return { rewind, nextAt: rewind ? -Infinity : now };
  }
}

class CodexCli extends SessionDockCli {
  constructor() {
    super('codex', 'Codex', 'i-codex', 'var(--codex)');
  }

  migrateQueuedMessages(items, fromVersion, _toVersion) {
    // v4 起 Codex 终端提交回执归服务端管理。浏览器旧副本没有交付凭据，
    // 全部丢弃；真实回执会随 /api/messages 或 SSE 重新同步回来。
    return fromVersion < 4 ? [] : super.migrateQueuedMessages(items);
  }

  normalizeQueuedText(value) {
    // Codex TUI 写 rollout 前会裁掉 prompt 首尾空白；内部空格和换行仍须精确。
    return String(value ?? '').trim();
  }

  queuedMessageLabel(item) {
    if (item?.state === 'aborted') return '已中断';
    if (item?.state === 'injecting' || item?.state === 'delivering') {
      return '正在写入终端';
    }
    if (item?.state === 'confirming') return '已送达终端';
    if (item?.state === 'failed' && +item?.attempts > 0) return '终端写入待核对';
    if (item?.state === 'failed') return '未写入终端';
    return '等待终端确认';
  }

  questionAnswerKeys(prompt, optionIndex) {
    const options = prompt?.questions?.[0]?.options || [];
    if (!options[optionIndex] || optionIndex >= 9) return null;
    // Command approvals are TUI-only and advertise stable mnemonic keys.  Use
    // those instead of assuming they share request_user_input's numeric menu.
    if (prompt?.kind === 'approval') {
      const key = options[optionIndex]?.key;
      return key ? [key] : null;
    }
    // Codex 的问题菜单会循环选择，不能照搬 Claude 的“多按 Up 夹到
    // 第一项”。菜单原生支持数字直选且立即提交，位置不受另一网页或
    // 原生终端先前移动光标的影响。request_user_input 目前最多三个选项。
    return [String(optionIndex + 1)];
  }

  repeatedEscape(now, previousAt, context = {}) {
    // Codex 官方语义：运行/询问中单 Esc 中断；空输入双 Esc 进入上一条
    // 用户消息的编辑与 fork 界面。
    if (context.busy || context.empty === false) {
      return { rewind: false, nextAt: -Infinity };
    }
    const rewind = now - previousAt <= 650;
    return { rewind, nextAt: rewind ? -Infinity : now };
  }

  // Codex 的 follow-up 已经写入当前 TUI，由 TUI 自己排队。Esc 只中断
  // 当前回合；服务端回执继续等待对应的原生用户记录。
}

class GrokCli extends SessionDockCli {
  constructor() {
    super('grok', 'Grok', 'i-grok', 'var(--grok)');
  }

  // Grok 暂无已验证的内存队列控制事件：只使用基类的正式消息对账，
  // 不把 Claude/Codex 的 Esc 假设套过来。
  queueAction(message) {
    return super.queueAction(message);
  }

  normalizeQueuedText(value) {
    return String(value ?? '').trim();
  }

  queuedTextMatches(pending, native) {
    const left = this.normalizeQueuedText(pending);
    const right = this.normalizeQueuedText(native);
    if (!left || !right) return false;
    if (left === right) return true;
    // Grok 走 /api/term/send 粘贴进 TUI。输入框里若有残留草稿，原生
    // user 记录会变成「残留前缀 + 网页正文」。精确相等会留下第二条
    // 排队气泡；只允许原生以网页正文为后缀，避免把中间碰巧相同的
    // 短句当成回执。
    return right.endsWith(left) && right.length > left.length;
  }
}

const SESSIONDOCK_CLIS = Object.freeze({
  claude: new ClaudeCli(),
  codex: new CodexCli(),
  grok: new GrokCli(),
});

function sessiondockCli(sourceOrUid) {
  const value = String(sourceOrUid || '');
  let source = value.split(':', 1)[0];
  // 首条原生记录落盘前，新建会话的 uid 是 tmux:sessiondock-<cli>-...。
  // 这个阶段也必须使用对应 CLI 的发送确认策略。
  if (source === 'tmux') {
    source = value.match(/^tmux:sessiondock-(claude|codex|grok)-/)?.[1] || source;
  }
  return SESSIONDOCK_CLIS[source] || null;
}

// 供 app.js、term.js 以及 headless 回归共同使用。
Object.assign(globalThis, {
  SessionDockCli, ClaudeCli, CodexCli, GrokCli, SESSIONDOCK_CLIS, sessiondockCli,
});
