

export function createQuestionRuntime() {
const questionFormDrafts = new Map();

const screenMenuTextDrafts = new Map();

function pendingHistoryQuestion(entry) {
  if (entry?.meta?.source !== 'codex' || entry?.activity?.state !== 'waiting') return null;
  const answered = new Set((entry.msgs || []).filter(m => ['answer','tool_result'].includes(m.role) && m.call_id).map(m => m.call_id));
  return [...(entry.msgs || [])].reverse().find(m => m.role === 'question' && m.call_id && !answered.has(m.call_id)) || null;
}

function pruneQuestionFormDrafts(uid, activeId = '') {
  const prefix = `${uid}\0`, keep = activeId ? `${prefix}${activeId}` : '';
  for (const key of questionFormDrafts.keys()) if (key.startsWith(prefix) && key !== keep) questionFormDrafts.delete(key);
}
return {questionFormDrafts,screenMenuTextDrafts,pendingHistoryQuestion,pruneQuestionFormDrafts};
}
