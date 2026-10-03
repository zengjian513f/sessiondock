export const viewKey = (uid, agent = null) => agent ? `${uid}::${agent}` : uid;
export const INCOMING_ROLES = new Set([
  'assistant', 'assistant·subagent', 'thinking', 'tool', 'tool_result', 'question',
]);
export const incomingCount = msgs => msgs.filter(m => m.counted !== false && INCOMING_ROLES.has(m.role)).length;
export const messageCount = msgs => msgs.filter(m => m.counted !== false).length;
export const entryTotal = entry => Number.isFinite(+entry?.total)
  ? +entry.total : messageCount(entry?.msgs || []);

export function normalizedQuestionAnswer(text) {
  const raw = String(text || '');
  if (/^aborted by user(?:\s|$)/i.test(raw.trim())
      || raw.startsWith("The user doesn't want to proceed with this tool use.")) {
    return '已取消回答';
  }
  try {
    const answers = JSON.parse(raw)?.answers;
    if (!answers || typeof answers !== 'object') return raw;
    const rows = Object.values(answers).map(answer => {
      const values = answer && typeof answer === 'object' ? answer.answers : answer;
      return Array.isArray(values) ? values.join('、') : String(values || '').trim();
    }).filter(Boolean);
    return rows.join('\n') || raw;
  } catch { return raw; }
}

/** 正文游标可能已经由并行 fetch 推进，但后到的 SSE 仍可能携带更新的活动态。
 *  活动态有自己的时间线：接收较新的状态，绝不让旧 Working 覆盖已中断。 */
export function activityFollows(current, incoming) {
  if (!current) return true;
  if (!incoming) return ['working', 'waiting'].includes(current.state);
  const currentAt = Date.parse(current.ts || '');
  const incomingAt = Date.parse(incoming.ts || '');
  if (Number.isFinite(currentAt) && Number.isFinite(incomingAt)) {
    return incomingAt >= currentAt;
  }
  const currentBusy = ['working', 'waiting'].includes(current.state);
  const incomingBusy = ['working', 'waiting'].includes(incoming.state);
  if (currentBusy !== incomingBusy) return currentBusy && !incomingBusy;
  return true;
}
