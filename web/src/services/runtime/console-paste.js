import * as SessionUi from '../../migration/session-ui'

import {clipboardAttachmentFiles} from '../../domain/composer/input.js'

export function createConsolePaste({settingsRuntime,terminal,composer,core,post}){
function consolePasteFiles(view, name, e) {
  const files = clipboardAttachmentFiles(e.clipboardData);
  if (!files.length) return;
  if (!settingsRuntime().consolePasteFilesEnabled()) {
    SessionUi.consoleToast('已忽略粘贴的文件；在 设置 › 功能 开启「控制台粘贴文件」后会存入会话目录');
    setTimeout(() => {
      if (document.querySelector('#console-toast')?.textContent.startsWith('已忽略粘贴的文件')) SessionUi.consoleToast('');
    }, 6000);
    return;
  }
  e.preventDefault();
  e.stopPropagation();
  if (terminal().state.name !== name || view.replay || view.ended || view.revoked) return;
  composer().whenPasteConfirmed(files, () => {
    const uid = view.bindingUid || terminal().state.uid || '';
    if (!uid) { SessionUi.appAlert('粘贴文件失败：这个终端还没有会话目录'); return; }
    // One paste after another keeps its order, and each paste is one batch.
    const previous = consolePasteJobs.get(name) || Promise.resolve();
    const job = previous.then(() => publishConsolePaste(view, name, uid, files));
    consolePasteJobs.set(name, job.catch(() => {}));
  });
}

const consolePasteJobs = new Map();

function consoleAttachmentPath(document) {
  const relative = String(document.relative_path || '').replace(/^\.[\\/]/, '');
  const windows = document.path_style === 'windows';
  if (!relative) return String(document.path || '');
  if (windows) {
    const path = '.\\' + relative.replace(/\//g, '\\');
    return /\s/.test(path) ? `"${path}"` : path;
  }
  return ('./' + relative).replace(/\s/g, ch => '\\' + ch);
}

async function publishConsolePaste(view, name, uid, files) {
  const paths = [];
  let attachmentId = null;
  SessionUi.consoleToast(files.length === 1 ? `正在保存 ${files[0].name || '附件'}…` : `正在保存 ${files.length} 个文件…`);
  try {
    for (const file of files) {
      if (!file.size) throw new Error(`「${file.name || '附件'}」为空`);
      if (file.size > composer().COMPOSER_MAX_FILE_BYTES) throw new Error(`「${file.name || '附件'}」超过 512 MB`);
      const url = new URL(core.environment.appUrl('api/session/attachment'));
      url.searchParams.set('uid', uid);
      url.searchParams.set('name', file.name || 'attachment');
      if (attachmentId) url.searchParams.set('id', attachmentId);
      const response = await core.network.fetch(url, {
        method: 'POST', body: file,
        headers: {'Content-Type': file.type || 'application/octet-stream', 'X-SessionDock-Page': core.environment.AUDIT_PAGE_ID},
      });
      let data = {};
      try { data = await response.json(); } catch { /* the status carries the failure */ }
      if (!response.ok || data.error) throw new Error(data.error || `HTTP ${response.status}`);
      attachmentId ||= data.attachment_id;
      paths.push(consoleAttachmentPath(data));
    }
  } catch (error) {
    SessionUi.consoleToast('');
    await SessionUi.appAlert('粘贴文件失败：' + (error.message || error));
    return;
  } finally {
    if (document.querySelector('#console-toast')?.textContent.startsWith('正在保存')) SessionUi.consoleToast('');
  }
  if (terminal().state.name !== name || view.ended || view.revoked) return;
  const text = paths.join(' ') + ' ';
  core.audit.browserAuditEvent?.('terminal.paste_files', {name, count: paths.length, attachment_id: attachmentId},
    null, {uid, connectionId: view.auditConnectionId || ''});
  try {
    const d = await post().post('api/term/send', terminal().termInputBody(name, {name, paste: text, uid}) || {name, paste: text});
    if (d.error) throw new Error(d.error);
  } catch (error) {
    await SessionUi.appAlert(`文件已保存到 ${paths.join(' ')}，但没有写进终端：` + (error.message || error));
  }
}

function bindFileDrop(zone, addFiles) {
  zone.addEventListener('dragenter', e => {
    if (e.dataTransfer?.types?.includes('Files')) zone.classList.add('dragover');
  });
  zone.addEventListener('dragover', e => {
    if (!e.dataTransfer?.types?.includes('Files')) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = 'copy';
  });
  zone.addEventListener('dragleave', e => {
    if (!zone.contains(e.relatedTarget)) zone.classList.remove('dragover');
  });
  zone.addEventListener('drop', e => {
    zone.classList.remove('dragover');
    const files = [...(e.dataTransfer?.files || [])];
    if (!files.length) return;
    e.preventDefault();
    addFiles(files);
  });
}
return {consolePasteFiles,consolePasteJobs,consoleAttachmentPath,publishConsolePaste,bindFileDrop}
}
