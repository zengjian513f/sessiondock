/** @param {import('../../domain/session-ui/types').SessionUiPresentation} ui */
export function createTrashController(bridge, ui) {
let trashScope=[],trashItems=[],trashBusy=false;
function openTrash() {
  const dlg = bridge.$('#trash-dialog');
  if (!dlg.open) dlg.showModal();
  loadTrash();
}

async function loadTrash({ keepNote = false } = {}) {
  if (!keepNote) setTrashNote('');   // 刷新列表不能把刚做完那件事的回执抹掉
  ui.trash = []; ui.trashEmpty = '正在读取回收站…';
  try {
    const scope = bridge.selectedNodeIds();
    const r = await fetch(bridge.appUrl('api/trash' + (bridge.HUB_MODE ? '?nodes=' + scope.join(',')
      : bridge.trashCapable() ? '?limit=200' : '')));
    const d = await r.json();
    if (!r.ok) throw new Error(d.error || r.status);
    bridge.applyNodeState(d, 'trash');
    trashScope = scope;
    trashItems = Array.isArray(d.items) ? d.items : [];
    renderTrash(d);
    return true;
  } catch (e) {
    trashItems = [];
    ui.trash = []; ui.trashEmpty = '读取失败';
    setTrashNote('读取回收站失败: ' + e.message, true);
    return false;
  }
}

function renderTrash(info) {
  const dir = info?.dir || '';
  const total = Number.isInteger(info?.count) ? info.count : trashItems.length;
  ui.trashSub = trashItems.length
    ? `${total} 个已删除会话 · 共 ${bridge.fmtSize(info?.size || 0)} · ${dir}`
      + (info?.next_cursor ? ` · 仅显示最近 ${trashItems.length} 条` : '')
    : `回收站是空的 · ${dir}`;
  ui.trashDisabled = !trashItems.length;
  ui.trash = trashItems.map(it=>({id:it.id,title:it.title,cwd:it.cwd,origin:it.origin,restorable:it.restorable,reason:it.reason,icon:bridge.SOURCES[it.source]?bridge.icon(it.source):'',when:bridge.fmtTime(it.deleted_at),sizeLabel:bridge.fmtSize(it.size),directory:bridge.nodeDirectory(it,34),originLabel:bridge.shortCwd(it.origin,200)}));
  ui.trashEmpty = '没有已删除的会话';
}
function setTrashNote(text,isError=false) {ui.trashNote=text || '';ui.trashError=!!text && isError;}

async function trashPost(path, body, pending) {
  trashBusy = true;
  ui.trashPending = pending || '';
  try {
    const r = await fetch(bridge.appUrl(path), {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
    const d = await r.json().catch(() => ({}));
    if (!r.ok) { setTrashNote(d.error || `请求失败: ${r.status}`, true); return null; }
    return d;
  } catch (e) {
    setTrashNote('请求失败: ' + e.message, true);
    return null;
  } finally {
    trashBusy = false;
    ui.trashPending = '';
  }
}

async function trashItemAction(e) {
  const btn = e.target.closest('button[data-act]');
  if (!btn || trashBusy) return;
  const id = btn.closest('.trash-item')?.dataset.id;
  const item = trashItems.find(x => x.id === id);
  if (!item) return;
  if (btn.dataset.act === 'restore') {
    const d = await trashPost('api/trash/restore', { id: item.id }, item.id + ':' + btn.dataset.act);
    if (!d) return;
    if (await loadTrash({ keepNote: true })) {
      setTrashNote(`已恢复「${item.title}」到 ${d.path}`);
    }
    bridge.cancelSearch(true);
    await bridge.loadSessions(true);       // 恢复的会话立即回到左侧列表
    return;
  }
  if (!await bridge.appConfirm(`彻底删除「${item.title}」?\n\n文件将从磁盘移除, 不可恢复。`)) return;
  const d = await trashPost('api/trash/purge', { id: item.id }, item.id + ':' + btn.dataset.act);
  if (!d) return;
  if (await loadTrash({ keepNote: true })) {
    setTrashNote(`已彻底删除「${item.title}」, 释放 ${bridge.fmtSize(d.freed || 0)}`);
  }
}

async function purgeAllTrash() {
  if (!trashItems.length || trashBusy) return;
  if (!await bridge.appConfirm(`清空回收站?\n\n将从磁盘彻底删除 ${trashItems.length} 个会话, 不可恢复。`)) return;
  const d = await trashPost('api/trash/purge' + (bridge.HUB_MODE ? '?nodes=' + trashScope.join(',') : ''),
    { all: true }, 'all');
  if (!d) return;
  const failed = (d.errors || []).length;
  if (await loadTrash({ keepNote: true })) {
    setTrashNote(`已彻底删除 ${d.removed || 0} 个会话, 释放 ${bridge.fmtSize(d.freed || 0)}`
      + (failed ? `; ${failed} 个失败: ${d.errors[0]}` : ''), !!failed);
  }
}



return {openTrash,loadTrash,purgeAllTrash,trashItemAction};
}
