// Pure extraction of the existing composer history, readiness, numbering and
// clipboard projections. Limits and browser input conventions are unchanged.
export function composerHistoryStamp(entry) {
  return `${entry?.end || 0}:${entry?.version?.head || ''}:${entry?.anchor || ''}`;
}

export function nativeComposerHistory(messages) {
  return (messages || []).filter(message =>
    ['user', 'command'].includes(message?.role)
    && message.counted !== false && String(message.text || '').trim()
  ).map((message, index) => ({
    id: `native-${index}`, text: String(message.text), ts: message.ts || null,
  }));
}

export function ensureComposerAttachmentNumbers(draft) {
  draft.attachments ||= [];
  const used = new Set();
  let next = Number.isInteger(draft.nextAttachmentNumber) && draft.nextAttachmentNumber > 0
    ? draft.nextAttachmentNumber : 1;
  for (const attachment of draft.attachments) {
    if (!Number.isInteger(attachment.number) || attachment.number < 1 || used.has(attachment.number)) {
      while (used.has(next)) next++;
      attachment.number = next++;
    }
    used.add(attachment.number);
    next = Math.max(next, attachment.number + 1);
  }
  draft.nextAttachmentNumber = next;
  return draft;
}

export function composerFileKind(file) {
  const prefix = String(file.type || '').split('/', 1)[0];
  return ['image', 'video', 'audio'].includes(prefix) ? prefix : 'file';
}

export function composerKindIcon(kind) {
  return { image: '▧', video: '▶', audio: '♪', file: '⌑' }[kind] || '⌑';
}

export function composerInputStatus(data) {
  const fallback = {state:'unknown', code:'input_check_pending', message:'正在检查 CLI 输入状态'};
  if (!data || typeof data !== 'object') return fallback;
  if (Object.prototype.hasOwnProperty.call(data, 'input')) {
    const input = data.input;
    if (!input || !['ready','starting','blocked','unknown'].includes(input.state)
        || typeof input.code !== 'string' || typeof input.message !== 'string') return fallback;
    if (input.state !== 'ready') return {state:input.state, code:input.code, message:input.message};
    if (data.ok === true && !data.error) return {state:input.state, code:input.code, message:input.message};
    return {state:'unknown', code:data.code || 'input_check_failed', message:data.error || '未确认 CLI 可输入，请切换终端检查'};
  }
  if (data.ok === true && !data.error) return {state:'ready', code:'', message:''};
  const code = typeof data.code === 'string' ? data.code : 'input_check_failed';
  const state = ['cli_starting','cli_catching_up','cli_pasting'].includes(code) ? 'starting'
    : code === 'cli_question' ? 'blocked' : 'unknown';
  return {state, code, message:typeof data.error === 'string' ? data.error : '未确认 CLI 可输入，请切换终端检查'};
}

export function composerInputAllowsSend(status) {
  return status?.state === 'ready';
}

export function screenMenuRevision(prompt) {
  return prompt?.revision || JSON.stringify([prompt?.questions, prompt?.text, prompt?.actions]);
}

export function clipboardAttachmentFiles(data) {
  const files = [];
  const mirrored = new Map();
  const keyOf = file => `${file.name}\0${file.type}\0${file.size}`;
  // Chromium 通常把文件放在 files；部分浏览器/桌面剪贴板只在 items
  // 暴露非图片文件（CSV 尤其常见）。files 作为主清单；items 中每个同名、
  // 同类型、同大小的项只抵消一个镜像。不能比较 lastModified：同一张图片的
  // 两个 Chromium File 对象会相差 1ms；按计数抵消又能保留真正的同名文件。
  for (const file of [...(data?.files || [])]) {
    if (!(file instanceof File)) continue;
    files.push(file);
    const key = keyOf(file);
    mirrored.set(key, (mirrored.get(key) || 0) + 1);
  }
  for (const item of [...(data?.items || [])]) {
    if (item.kind !== 'file') continue;
    const file = item.getAsFile?.();
    if (!(file instanceof File)) continue;
    const key = keyOf(file);
    const copies = mirrored.get(key) || 0;
    if (copies) {
      if (copies === 1) mirrored.delete(key);
      else mirrored.set(key, copies - 1);
    } else {
      files.push(file);
    }
  }
  return files;
}

export function clipboardDirectoryNames(data) {
  const names = [];
  for (const item of [...(data?.items || [])]) {
    if (item.kind !== 'file') continue;
    const getEntry = item.getAsEntry || item.webkitGetAsEntry;
    let entry = null;
    try { entry = getEntry?.call(item); } catch { /* 浏览器不允许读取该项 */ }
    if (entry?.isDirectory) names.push(entry.name || '文件夹');
  }
  return names;
}

export function clipboardCsvFile(data, callback) {
  const csvTypes = new Set([
    'text/csv', 'text/comma-separated-values', 'application/csv',
    'application/vnd.ms-excel',
  ]);
  const item = [...(data?.items || [])].find(x =>
    x.kind === 'string' && csvTypes.has(String(x.type || '').toLowerCase()));
  if (!item) return false;
  const mime = String(item.type || 'text/csv').toLowerCase();
  const accept = text => {
    if (typeof text === 'string' && text.length) {
      callback(new File([text], 'clipboard.csv', { type: mime, lastModified: Date.now() }));
    }
  };
  // getData 是同步的，但有些 DataTransfer 实现只支持 getAsString。
  const immediate = data.getData?.(item.type);
  if (immediate) accept(immediate);
  else item.getAsString?.(accept);
  return true;
}
