// @ts-nocheck
// Existing conversation planning and display algorithms; no transport or DOM ownership.
let compactTurns = true;
export function configurePlanning(value) { compactTurns = value; }
export const TOOL_ROLES = new Set(['tool', 'tool_result']);
export const SEARCH_ROLES = new Set(['user', 'assistant', 'user·subagent', 'assistant·subagent',
                              'thinking', 'question', 'answer', 'command']);
export const TURN_START_ROLES = new Set(['user', 'user·subagent']);
export const GROUP_MIN = 2;
export const MESSAGE_TIME_GAP_MS = 5 * 60 * 1000;
export const MESSAGE_TIME_CADENCE_MS = 20 * 60 * 1000;

export const isGroupableTool = m => TOOL_ROLES.has(m?.role) && !m.changes?.length;

/** 调用与其输出按 call_id 就近配对成一个视觉单元(对标 codex TUI 的 `$ 命令 + 输出`)。
 *  只在本批消息内配对；增量批里落单的输出保持原样渲染，不会丢。 */
export function pairTools(msgs) {
  const out = [], open = new Map();
  for (const m of msgs) {
    if (m.role === 'tool') {
      const copy = { ...m };
      out.push(copy);
      if (m.call_id) open.set(m.call_id, copy);
      continue;
    }
    if (m.role === 'tool_result' && m.call_id && open.has(m.call_id)) {
      const owner = open.get(m.call_id);
      open.delete(m.call_id);
      owner.result = m;
      continue;
    }
    out.push(m);
  }
  return out;
}

/** 先算分组(纯计算, 很快), 再分批建 DOM —— 分批不会把一个组切成两半。 */
export function planMessages(msgs, { openTail = false } = {}) {
  const plan = [];
  let run = [];
  const flush = open => {
    if (run.length >= GROUP_MIN) plan.push({ g: run, open: !!open });
    else for (const m of run) plan.push({ m });
    run = [];
  };
  for (const m of pairTools(msgs)) {
    // 调用与结果跨增量批次时，空结果配不到上面的调用。它仍参与消息计数，
    // 但不能凭空生成一块黑色空卡片。
    if (m.role === 'tool_result' && !String(m.text || '').trim()
        && !m.media?.length && !m.changes?.length) {
      flush(false);
      plan.push({ m: { ...m, silent: true } });
      continue;
    }
    // 文件修改本身是用户关心的工作记录，始终作为可见卡片留在时间线；
    // 普通工具协议继续按原规则合并折叠。
    if (isGroupableTool(m)) { run.push(m); continue; }
    flush(false);
    plan.push({ m });
  }
  flush(openTail);
  return plan;
}

export const baseMessageRole = role => String(role || '').split('·', 1)[0];
export const isTurnStart = m => TURN_START_ROLES.has(m?.role);
export const sameNativeTurn = (a, b) => a?.turn_id != null && b?.turn_id != null
  && String(a.turn_id) === String(b.turn_id);
export const isTurnAssistant = m => baseMessageRole(m?.role) === 'assistant';
export const isFinalAssistant = m => isTurnAssistant(m)
  && ['final', 'final_answer', 'end_turn'].includes(m?.phase);
// rename/compact 等不计入消息数的辅助记录可能写在 final 之后；它们继续留在
// 时间线，但不应让前面的原生最终答复失去“结论”资格。
export const isPassiveTurnTail = m => m?.counted === false;

/** 增量中断状态来自 activity 包，可能与最后一条 commentary 分批到达。 */
export function markInterruptedTurn(messages, activity) {
  const turnId = activity?.state === 'aborted' && activity?.turn_id != null
    ? String(activity.turn_id) : '';
  if (!turnId) return false;
  for (let i = (messages || []).length - 1; i >= 0; i--) {
    const message = messages[i];
    if (String(message?.turn_id || '') !== turnId || !isTurnAssistant(message)) continue;
    if (!isFinalAssistant(message)) {
      message.interrupted = true;
      message.interrupt_reason = activity.reason || '本轮在最终答复前被中断';
      return true;
    }
    return false;
  }
  return false;
}

/** 找一轮中需要永久露出的结论块。原生 final 标记优先；明确中断的轮次
 *  保留最后一条 commentary 作为末次状态。老会话只有在回合已确定结束、
 *  且最后一个有效节点就是 assistant 时才回退到“最后一条”。 */
export function turnConclusion(body, { complete = false, interrupted = false } = {}) {
  let meaningfulEnd = body.length;
  while (meaningfulEnd && isPassiveTurnTail(body[meaningfulEnd - 1])) meaningfulEnd--;
  if (!meaningfulEnd) return null;
  // 主助手的第一条原生 final 就是它对本轮输入的答复。final 之后同一轮里还会
  // 出现内容的只有两种情形：Claude 的后台 task-notification（时间线里转成不
  // 打扰主线的 task 事件，后面跟一条监控短报），以及 Stop hook 拒绝收尾后被
  // 逼出的工具调用和一条短补充。两者都不能反过来把前面的 final 降成可折叠
  // 的“进展”——否则真正的结论被折进过程合集，页面只露出末尾几行补充。
  // final 之后的部分标记 continued，交给调用方作为一段新的过程继续规划。
  for (let i = 0; i < meaningfulEnd; i++) {
    if (body[i]?.role !== 'assistant' || !isFinalAssistant(body[i])) continue;
    let end = i + 1;
    while (end < meaningfulEnd && isFinalAssistant(body[end])) end++;
    return {start: i, end, continued: end < meaningfulEnd};
  }
  const last = body[meaningfulEnd - 1];
  if (isFinalAssistant(last)) {
    let start = meaningfulEnd - 1;
    while (start > 0 && isFinalAssistant(body[start - 1])) start--;
    return {start, end: meaningfulEnd};
  }
  if (complete && interrupted) {
    for (let i = meaningfulEnd - 1; i >= 0; i--) {
      if (!isTurnAssistant(body[i]) || !body[i]?.interrupted) continue;
      return {start: i, end: i + 1, tailStart: meaningfulEnd,
              inferred: true, interrupted: true};
    }
  }
  if (complete && !interrupted && isTurnAssistant(last) && !last.phase) {
    return {start: meaningfulEnd - 1, end: meaningfulEnd, inferred: true};
  }
  return null;
}

export function visiblePlanSize(plan) {
  return plan.reduce((n, item) => n + (item.m?.silent ? 0 : (item.m || item.g ? 1 : 0)), 0);
}

export function turnKey(messages, conclusion) {
  const values = [...messages, ...(conclusion || [])];
  const native = values.find(m => m?.turn_id)?.turn_id;
  if (native) return native;
  const first = messages[0] || conclusion?.[0] || {};
  return `${first.ts || 'turn'}:${String(first.text || '').slice(0, 80)}`;
}

/** 单轮外层折叠。用户输入和最终结论仍是普通顶层气泡，中间过程才进入合集；
 *  合集内部继续复用 planMessages 的工具配对/分组规则。 */
export function planTurnSegment(messages, promptEnd,
                         {complete = false, foldable = complete, openTail = false} = {}) {
  const prompts = messages.slice(0, promptEnd);
  const body = messages.slice(promptEnd);
  const conclusion = turnConclusion(body, {
    complete, interrupted: messages.some(m => m?.interrupted),
  });
  // 历史段可能因用户在同一次原生 turn 中追加要求，或上一轮被中断，而没有
  // 自己的 final。它已经被后续 user 明确封口，仍应作为过程折叠；只是不能
  // 把最后一条 commentary 猜成结论。尚在增长的尾段继续完整铺开。
  if (!conclusion && !foldable) return planMessages(messages, {openTail});
  // 中断时最后一条助手状态后面还可能有工具结果。它们仍属于过程；把这条
  // 状态提升到合集后作为“末次进展”，既不丢工具，也不把工具散回顶层。
  const process = conclusion?.interrupted
    ? [...body.slice(0, conclusion.start),
       ...body.slice(conclusion.end, conclusion.tailStart)]
    : conclusion ? body.slice(0, conclusion.start) : body;
  const processPlan = planMessages(process);
  const finalBlock = conclusion ? body.slice(conclusion.start, conclusion.end) : [];
  const rest = conclusion ? body.slice(conclusion.tailStart ?? conclusion.end) : [];
  // 原生 final 之后被追加的部分（task 监控短报、Stop hook 逼出的工具调用与
  // 补充说明）自成一段过程：够长就折成第二个合集并露出它自己的收尾，只有
  // 一两项时平铺。中断轮与 rename/compact 之类的被动尾巴仍按原样平铺。
  const tail = conclusion?.continued
    ? planTurnSegment(rest, 0, {complete, foldable, openTail})
    : planMessages(rest);
  // 一项换成一项不会节省空间，还会徒增一次点击。
  const processSize = visiblePlanSize(processPlan);
  // 思考例外：它在主线上整段铺开，折进合集才只占一行摘要。OpenCode 这类
  // 显示推理原文的 CLI 常见“一段思考 + 结论”的回合，不能让思考散在主线。
  const loneThinking = processSize === 1
    && processPlan.some(item => item.m?.role === 'thinking' && !item.m.silent);
  // 中断轮已经要保留末次状态；即便只剩一个工具单元，也应进过程合集，
  // 否则恰好较短的中断轮会再次把工具卡散在对话主线里。
  if (!processSize || (processSize < 2 && !conclusion?.interrupted && !loneThinking)) {
    if (!conclusion?.continued) return planMessages(messages);
    return [...planMessages(messages.slice(0, promptEnd + conclusion.end)), ...tail];
  }
  return [
    ...prompts.map((m, i) => ({m, sealedTurnHead: i === 0})),
    {turn: {items: process, plan: processPlan,
            key: turnKey(messages, finalBlock), hasConclusion: !!conclusion,
            inferred: !!conclusion?.inferred,
            interrupted: !!conclusion?.interrupted},
     open: !compactTurns},
    ...planMessages(finalBlock),
    ...tail,
  ];
}

export function planTurn(messages, options = {}) {
  if (!messages.length || !isTurnStart(messages[0])) {
    return planMessages(messages, {openTail: options.openTail});
  }
  // Claude 会把同一次含文字/图片的 user 记录拆成多个规范化气泡。相邻且
  // turn_id 相同的部分是一份输入，全部留在顶层，不能把图片折进“过程”。
  let promptEnd = 1;
  while (promptEnd < messages.length && isTurnStart(messages[promptEnd])
         && sameNativeTurn(messages[0], messages[promptEnd])) promptEnd++;
  return planTurnSegment(messages, promptEnd, options);
}

/** 历史缺口两侧会分别调用，绝不跨缺口猜轮次。answer 是代理提问的回答，
 *  留在同一轮过程内；只有真正的 user/user·subagent 开新轮。 */
export function planTurns(msgs, {
  openTail = false, tailComplete = false, foldTail = tailComplete,
} = {}) {
  const out = [];
  let start = msgs.findIndex(isTurnStart);
  // 窗口缺口可能截在一轮正中：缺口前的尾段可以折叠，但不能据此猜结论；
  // 缺口后的前缀若被下一条 user 封口，也按无输入的历史过程片段处理。
  if (start < 0) {
    return planTurnSegment(msgs, 0, {
      complete: tailComplete, foldable: foldTail, openTail,
    });
  }
  out.push(...planTurnSegment(msgs.slice(0, start), 0, {
    complete: true, foldable: true,
  }));
  while (start < msgs.length) {
    let next = start + 1;
    while (next < msgs.length && isTurnStart(msgs[next])
           && sameNativeTurn(msgs[start], msgs[next])) next++;
    while (next < msgs.length && !isTurnStart(msgs[next])) next++;
    const historical = next < msgs.length;
    out.push(...planTurn(msgs.slice(start, next), {
      complete: historical || tailComplete,
      foldable: historical || foldTail,
      openTail: !historical && openTail,
    }));
    start = next;
  }
  return out;
}

/** 一个工具卡可能同时包含调用和结果，工具组又包含多张卡。
 *  分隔线用这个视觉单元的最早/最晚时间，不会把一次长时间工具调用
 *  误判成与下一条消息的空档。 */
export function messageTimeRange(messages) {
  const values = [];
  for (const message of messages || []) {
    for (const item of [message, message?.result]) {
      const value = Date.parse(item?.ts || '');
      if (Number.isFinite(value)) values.push(value);
    }
  }
  return values.length ? {start: Math.min(...values), end: Math.max(...values)} : null;
}


export const OUT_LINES = 8;
export const OUT_CHARS = 1600;
export function outputStats(t) {
  const body = String(t || '').replace(/\n+$/, '');
  return { lines: body ? body.split('\n').length : 0, chars: String(t || '').length };
}

export function outPreviewInfo(t) {
  const body = t.replace(/\n+$/, '');
  const lines = body ? body.split('\n') : [];
  let preview = lines.length > OUT_LINES ? lines.slice(0, OUT_LINES).join('\n') : t;
  const lineCut = lines.length > OUT_LINES;
  if (preview.length > OUT_CHARS) preview = preview.slice(0, OUT_CHARS);
  const truncated = preview !== t;
  return {
    text: truncated ? preview.replace(/\s+$/, '') + '\n…' : t,
    omittedLines: lineCut ? Math.max(0, outputStats(t).lines - OUT_LINES) : 0,
    omittedChars: truncated ? Math.max(0, t.length - preview.length) : 0,
  };
}

export function toolOutputPath(m) {
  const name = String(m?.name || '').toLowerCase();
  if (!/(?:^|[_:./-])(?:read|read_file|notebookread|open)$/.test(name)) return '';
  const raw = String(m?.text || '');
  try {
    const value = JSON.parse(raw);
    for (const key of ['file_path', 'path', 'filename']) {
      if (typeof value?.[key] === 'string') return value[key];
    }
  } catch { /* 某些适配器传的是 Python repr 或纯命令，继续用文本提取 */ }
  const field = raw.match(/["'](?:file_path|path|filename)["']\s*[:=]\s*["']([^"']+)["']/);
  if (field) return field[1];
  const summary = String(m?.summary || '');
  const match = summary.match(/(?:^|\s)([.~\w/-]+\.[A-Za-z0-9]+)(?=\s|$|[,:;)]|$)/);
  return match?.[1] || '';
}


export const CHANGE_LABEL = {
  add: '新建', update: '修改', delete: '删除', edit: '修改', write: '写入',
};

export function diffKind(line) {
  if (/^(diff --git |index |---|\+\+\+|\*\*\*)/.test(line)) return 'meta';
  if (line.startsWith('@@')) return 'hunk';
  if (line.startsWith('+')) return 'add';
  if (line.startsWith('-')) return 'del';
  return 'ctx';
}

export function diffCodePath(lines) {
  for (const prefix of ['+++ ', '--- ']) {
    const header = lines.find(line => line.startsWith(prefix));
    if (!header) continue;
    const path = header.slice(prefix.length).split('\t', 1)[0].trim().replace(/^(?:a|b)\//, '');
    if (path && path !== '/dev/null') return path;
  }
  return '';
}

export function diffCodeParts(line, kind) {
  if (kind === 'add' || kind === 'del') return {marker: line[0], source: line.slice(1)};
  if (kind === 'ctx') return {marker: line.startsWith(' ') ? ' ' : '', source: line.startsWith(' ') ? line.slice(1) : line};
  return null;
}


export function diffSides(change) {
  const before = [], after = [];
  for (const line of String(change.patch || '').split('\n')) {
    if (/^(---|\+\+\+|\*\*\*)/.test(line)) continue;
    if (line.startsWith('@@')) {
      before.push({ text: line, kind: 'meta' });
      after.push({ text: line, kind: 'meta' });
    } else if (line.startsWith('+')) {
      after.push({ text: line.slice(1), kind: 'add' });
    } else if (line.startsWith('-')) {
      before.push({ text: line.slice(1), kind: 'del' });
    } else {
      const text = line.startsWith(' ') ? line.slice(1) : line;
      before.push({ text, kind: 'ctx' });
      after.push({ text, kind: 'ctx' });
    }
  }
  return { before, after };
}


export function turnProcessSummary(items) {
  const assistant = items.filter(isTurnAssistant).length;
  const thinking = items.filter(m => m.role === 'thinking').length;
  const calls = items.filter(m => m.role === 'tool').length;
  const orphanResults = calls ? 0 : items.filter(m => m.role === 'tool_result').length;
  const questions = items.filter(m => m.role === 'question').length;
  const changes = items.flatMap(m => Array.isArray(m.changes) ? m.changes : []);
  const paths = [...new Set(changes.map(change => change.path).filter(Boolean))];
  const added = changes.reduce((n, change) => n + (+change.added || 0), 0);
  const removed = changes.reduce((n, change) => n + (+change.removed || 0), 0);
  const errors = items.filter(m => m.error || (Number.isFinite(+m.exit_code) && +m.exit_code !== 0)).length;
  const times = items.flatMap(m => {
    const value = Date.parse(m.ts || '');
    return Number.isFinite(value) ? [value] : [];
  });
  const duration = times.length > 1 ? Math.max(...times) - Math.min(...times) : 0;
  const stats = [];
  if (assistant) stats.push(`${assistant} 条进展`);
  if (thinking) stats.push(`${thinking} 段思考`);
  if (calls || orphanResults) stats.push(`🔧 ${calls || orphanResults}`);
  if (paths.length) stats.push(`修改 ${paths.length} 个文件${added || removed ? ` +${added} −${removed}` : ''}`);
  if (questions) stats.push(`${questions} 次确认`);
  if (errors) stats.push(`⚠ ${errors}`);
  if (duration >= 1000) stats.push(formatDuration(duration));
  if (!stats.length) stats.push(`${items.length} 条记录`);
  return {stats, paths, errors};
}


export function formatDuration(ms) {
  let seconds = Math.max(0, Number(ms) || 0) / 1000;
  if (seconds < 10) return `${seconds.toFixed(seconds < 1 ? 1 : 0)} 秒`;
  seconds = Math.round(seconds);
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const rest = seconds % 60;
  return [hours ? `${hours} 小时` : '', minutes ? `${minutes} 分` : '',
          rest || (!hours && !minutes) ? `${rest} 秒` : ''].filter(Boolean).join(' ');
}
