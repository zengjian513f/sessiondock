import * as SessionUi from '../../migration/session-ui'
import {viewKey,activityFollows,normalizedQuestionAnswer,incomingCount,messageCount,entryTotal} from '../../domain/runtime/messages.js'

// Packet acceptance and mutation belong to one service. The view only receives
// an accepted snapshot or suffix, never owns checkpoints or delivery state.
export function createDiffService(environment, state, messageCache, index, sync, conversation, composer, terminal, presentation) {
  const {capabilities, MOBILE} = environment;
  const {selection, catalog, unread, search} = state;
  const {cache} = messageCache;
  const cachePut = (...args) => messageCache.cachePut(...args);
  const {indexedSessions} = index;
  const {migrationReadPaused, scheduleDiffRecovery} = sync;
  const {messageIndex, markInterruptedTurn} = conversation;
  const {applyCliState} = composer;
  const {revealConversationForPrompt} = terminal;
  const {sealTurnTail, renderConversationTail, renderSessionAction, renderSide, head,
    layoutSessionHead, paintTurn, renderSession, addUnread, appendMessages,
    markMatches, updateMatchNav, sidebarTextSelectionProtected, refreshSidebarRows} = presentation;
  const $ = selector => document.querySelector(selector);
function applyCoveredActivity(uid, agent, entry, data) {
  if (!data.activity_changed || !activityFollows(entry.activity, data.activity)) return;
  entry.activity = data.activity;
  markInterruptedTurn(entry.msgs, entry.activity);
  if (selection.sel !== uid || selection.agent !== agent) return;
  const box = $('#msgs');
  if (box && entry.activity?.state !== 'working') sealTurnTail(box, entry);
  renderConversationTail(entry.activity, uid);
}

function applyMigrationMeta(uid, agent, entry, meta) {
  if (capabilities.config.backend !== 'rust' || !meta
      || meta.uid !== uid || (meta.agent_id || null) !== agent) return;
  const key = m => JSON.stringify([m.title, m.parent_title, m.sid, m.agent_type,
    m.cwd, m.model, !!m.starred, m.fork_parent_visible,
    m.nest_parent || null, m.group || null,
    (m.agent_items || []).map(a => [a.id, a.title, a.type])]);
  const changed = key(entry.meta) !== key(meta);
  entry.meta = meta;
  if (!agent) {
    const listed = indexedSessions().byUid.get(uid);
    // Cursor/size updates do not change membership, ordering, nesting or
    // filters; migration_warnings 同样只出现在详情里，左栏从不展示（docs/history-pages.md）。
    // Keep the indexed row object used by the resolved sidebar tree;
    // replace the array normally for every other kind of metadata change.
    if (listed && JSON.stringify(listed) !== JSON.stringify(meta)) {
      const local = Object.keys({...listed, ...meta}).every(field =>
        field === 'cursor' || field === 'size' || field === 'migration_warnings'
          || JSON.stringify(listed[field]) === JSON.stringify(meta[field]));
      if (local) {
        const sizeChanged = listed.size !== meta.size;
        for (const field of ['cursor', 'size', 'migration_warnings']) {
          if (Object.hasOwn(meta, field)) listed[field] = meta[field];
          else delete listed[field];
        }
        if (sizeChanged) {
          const node = $('#side').querySelector(`.item:not(.agent)[data-uid="${CSS.escape(uid)}"]`);
          if (node?._nestRow) {
            node._signature = null;
            if (!sidebarTextSelectionProtected()) refreshSidebarRows(uid);
          }
        }
      } else catalog.sessions = catalog.sessions.map(row => row.uid === uid ? {...meta} : row);
    }
  }
  if (!changed) {
    // The first native message changes an assigned launch from unused to
    // populated even when its title and other header fields stay the same.
    if (selection.sel === uid && !selection.agent) renderSessionAction(meta);
    return;
  }
  if (!agent) renderSide();
  if (selection.sel !== uid || selection.agent !== agent) return;
  const oldHead = $('#detail > .dhead');
  if (!oldHead) return;
  const wasOpen = oldHead.querySelector('#session-view-menu')?.hidden === false;
  const next = head(meta, entryTotal(entry));
  if (wasOpen) {
    SessionUi.restoreViews();
  }
  oldHead.replaceWith(next);
  layoutSessionHead();
}

async function applyDiff(uid, data, bytes = 0, agent = null) {
  try { return await applyDiffPacket(uid, data, bytes, agent); }
  // activity 与 CLI 画面都随数据包到达，左栏和会话头的回合状态跟着重画。
  finally { if (!agent) paintTurn(uid); }
}

async function applyDiffPacket(uid, data, bytes = 0, agent = null) {
  const key = viewKey(uid, agent);
  if (migrationReadPaused(uid, agent)) return 0;
  const e = cache.get(key);
  if (!e) return 0;
  if (data.prompt_only) {
    if (!agent && Object.prototype.hasOwnProperty.call(data, 'prompt')) {
      e.prompt = data.prompt || null;
      revealConversationForPrompt?.(uid, e.prompt);
    }
    if (selection.sel === uid && !selection.agent) renderConversationTail(e.activity, uid);
    return 0;
  }
  if (data.cli_only) {
    // The session's CLI state changed (input readiness, editor text, queued
    // sends) without new records (docs/cli-state.md).
    if (!agent && Object.prototype.hasOwnProperty.call(data, 'cli')) {
      e.cli = data.cli || null;
      applyCliState?.(uid, e.cli);
    }
    return 0;
  }
  // 正文 diff 受游标约束。SSE 与兜底拉取可能同时从同一旧游标出发；
  // 乱序包必须在修改 outbox、prompt 或乐观消息之前丢弃，否则正文没被
  // 接收，发送占位却已先清掉。
  if (!data.reset && data.start !== e.end) {
    const packetStart = Number(data.start), packetEnd = Number(data.end);
    const currentEnd = Number(e.end);
    if (Number.isFinite(packetStart) && Number.isFinite(packetEnd)
        && Number.isFinite(currentEnd)
        && packetStart < currentEnd && packetEnd <= currentEnd) {
      // SSE 与主动 fetch 从同一旧游标出发时，后到者可能是一份已被前者
      // 完整覆盖的重复正文。活动态使用独立修订，仍须接收其中较新的
      // aborted/failed；否则页面要等兜底拉取才会清掉 Working。
      applyCoveredActivity(uid, agent, e, data);
      return 0;
    }
    scheduleDiffRecovery(uid, agent);
    return 0;
  }
  if (!agent && Object.prototype.hasOwnProperty.call(data, 'prompt')) {
    e.prompt = data.prompt || null;
    revealConversationForPrompt?.(uid, e.prompt);
  }
  if (!agent && Object.prototype.hasOwnProperty.call(data, 'cli')) {
    e.cli = data.cli || null;
    applyCliState?.(uid, e.cli);
  }
  const questionCalls = data.reset ? new Set() : messageIndex(e.msgs).questions;
  for (const message of data.messages || []) {
    if (message.role === 'question' && message.call_id) questionCalls.add(message.call_id);
  }
  data.messages = (data.messages || []).map(m => {
    if (m.role !== 'tool_result' || !questionCalls.has(m.call_id)) return m;
    return { ...m, role: 'answer', name: m.name || 'AskUserQuestion',
      text: normalizedQuestionAnswer(m.text) };
  });
  if (data.reset) {                         // 回滚 / 重写过, 缓存作废
    markInterruptedTurn(data.messages, data.activity);
    cachePut(key, { meta: data.meta, msgs: data.messages, version: data.version,
                    end: data.end, anchor: data.anchor, activity: data.activity, bytes,
                    prompt: data.prompt || null, cli: data.cli ?? null,
                    total: data.message_total, partial: data.partial || null });
    unread.cursors.set(key, { end: data.end, head: data.version.head, anchor: data.anchor });
    if (selection.sel === uid && selection.agent === agent) {
      // Rust keeps SSE live during explicit window reloads and native resets.
      // Publish the new snapshot together with any suffix/activity accepted
      // while this renderer yields; the existing render path is retained.
      const options = capabilities.config.backend === 'rust'
        ? {historyPageEntry: cache.get(key)} : {};
      await renderSession(data.meta, data.messages, data.activity, options);
    }
    return data.messages.length;
  }
  e.version = data.version;
  e.end = data.end;
  e.anchor = data.anchor;
  unread.cursors.set(key, { end: data.end, head: data.version.head, anchor: data.anchor });
  e.bytes += bytes;
  applyMigrationMeta(uid, agent, e, data.meta);
  if (data.activity_changed) {
    e.activity = data.activity;
    // turn_aborted 可能单独成为一个无正文的 SSE 包，也可能与末批正文一起
    // 到达；两边都补标，不能依赖恰好落在同一次 JSONL 增量读取中。
    markInterruptedTurn(e.msgs, e.activity);
    markInterruptedTurn(data.messages, e.activity);
  }
  else if (e.activity?.state === 'waiting' && data.messages.some(
      m => m.role === 'tool_result' || m.role === 'answer')) {
    // 问题和回答可能分属两次增量读取，第二次已没有 call_id 映射。
    const answerAt = data.messages.findIndex(m => m.role === 'tool_result');
    data.messages = data.messages.map((m, i) => i === answerAt && m.role === 'tool_result'
      ? { ...m, role: 'answer' } : m);
    e.activity = { role: 'status', state: 'working', text: 'working', ts: new Date().toISOString() };
  }
  if (!data.messages.length) {
    if (selection.sel === uid && selection.agent === agent) {
      const box = $('#msgs');
      if (box && e.activity?.state !== 'working') sealTurnTail(box, e);
      renderConversationTail(e.activity, uid);
    }
    return 0;
  }
  e.total = entryTotal(e) + messageCount(data.messages);
  for (const message of data.messages) e.msgs.push(message);
  const incoming = incomingCount(data.messages);
  const detailVisible = selection.sel === uid && selection.agent === agent
    && (!MOBILE.matches || document.body.classList.contains('mobile-detail'));
  if (incoming && !detailVisible) addUnread(uid, incoming);
  if (selection.sel !== uid || selection.agent !== agent) return data.messages.length;
  const box = $('#msgs');
  if (!box) return data.messages.length;
  const built = appendMessages(box, data.messages, null,
    {openTail: e.activity?.state === 'working'});
  const sealed = sealTurnTail(box, e);
  if (!sealed) {
    built.forEach(markMatches);
    if (search.term) updateMatchNav();
  }
  renderConversationTail(e.activity, uid);
  SessionUi.messageCount(entryTotal(e));
  return data.messages.length;
}

/** 兜底用的主动拉取。正常情况下更新由服务端 SSE 推过来, 这里只在
 *  连接还没建起来或断了的时候补一手。 */
  return {applyCoveredActivity, applyMigrationMeta, applyDiff, applyDiffPacket};
}
