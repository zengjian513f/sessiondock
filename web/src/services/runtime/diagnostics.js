import {viewKey} from '../../domain/runtime/messages.js'

// Receipts, observers and long-frame attribution retain the established cadence.
export function createDiagnostics(environment, selection, terminal, messageCache, audit) {
  const {APP_BASE, MOBILE, appUrl, layoutTier, capabilities, network} = environment;
  const {cache} = messageCache;
  const browserAuditEvent=(...args)=>audit.browserAuditEvent(...args),flushBrowserAuditBeacon=(...args)=>audit.flushBrowserAuditBeacon(...args);
  const $ = selector => document.querySelector(selector);
const LONG_FRAME_MIN_MS = 1000;
const LONG_FRAME_MAX_EVENTS = 60;
let longFrameCount = 0;
function longFrameEvent(entry) {
  const base = APP_BASE.href;
  const scripts = [...(entry.scripts || [])].slice(0, 8).map(script => ({
    invoker: String(script.invoker || '').slice(0, 200), invoker_type: script.invokerType || '',
    function: String(script.sourceFunctionName || '').slice(0, 120),
    url: String(script.sourceURL || '').replace(base, ''), char: script.sourceCharPosition ?? null,
    duration_ms: Math.round(script.duration || 0),
    forced_layout_ms: Math.round(script.forcedStyleAndLayoutDuration || 0),
    pause_ms: Math.round(script.pauseDuration || 0),
  }));
  const end = entry.startTime + entry.duration;
  const memory = performance.memory;
  const term = typeof terminal.state === 'undefined' ? null
    : {name: terminal.state.name, views: terminal.state.views?.size ?? 0, connected: terminal.state.ws?.readyState ?? null};
  return {
    event: 'main_thread.long_frame',
    ts: new Date(performance.timeOrigin + entry.startTime).toISOString(),
    uid: selection.sel ?? '', trace_id: '', request_id: '', connection_id: '', severity: 'warning',
    data: {
      duration_ms: Math.round(entry.duration), blocking_ms: Math.round(entry.blockingDuration || 0),
      render_ms: entry.renderStart ? Math.round(end - entry.renderStart) : 0,
      style_layout_ms: entry.styleAndLayoutStart ? Math.round(end - entry.styleAndLayoutStart) : 0,
      scripts,
      heap_mb: memory ? [memory.usedJSHeapSize, memory.totalJSHeapSize, memory.jsHeapSizeLimit]
        .map(bytes => Math.round(bytes / 1048576)) : null,
      dom_nodes: document.getElementsByTagName('*').length,
      selected: selection.sel, agent: selection.agent, visibility: document.visibilityState,
      audit_queue: audit.queueLength, terminal: term,
    },
    content: null,
  };
}
function observeLongFrames() {
  if (!capabilities.allows('audit')) return;
  if (!globalThis.PerformanceObserver?.supportedEntryTypes?.includes('long-animation-frame')) return;
  const observer = new PerformanceObserver(list => {
    if (network.paused) return;
    for (const entry of list.getEntries()) {
      if (entry.duration < LONG_FRAME_MIN_MS || longFrameCount >= LONG_FRAME_MAX_EVENTS) continue;
      longFrameCount += 1;
      try {
        const event = longFrameEvent(entry);
        const sent = navigator.sendBeacon?.(appUrl('api/audit/browser'),
          new Blob([audit.auditPayload([event])], {type: 'application/json'}));
        if (!sent) browserAuditEvent(event.event, event.data, null, {severity: event.severity});
      } catch { /* 诊断不影响页面 */ }
    }
  });
  observer.observe({type: 'long-animation-frame', buffered: true});
}

function browserStateSnapshot(reason = '') {
  const box = $('#msgs');
  const nodes = box ? [...box.querySelectorAll('.msg, #activity')].slice(-40) : [];
  const entry = selection.sel ? cache.get(viewKey(selection.sel, selection.agent)) : null;
  const termState = typeof terminal.state === 'undefined' ? null : {
    name: terminal.state.name, uid: terminal.state.uid, mode: terminal.state.mode,
    visible: !$('#termpane')?.classList.contains('hidden'),
    connected: terminal.state.ws?.readyState ?? null,
    frozen: (terminal.state.list || []).find(row => row.uid === selection.sel)?.frozen ?? null,
  };
  return {
    data: {
      reason, selected: selection.sel, agent: selection.agent, mobile: MOBILE.matches,
      mobile_detail: document.body.classList.contains('mobile-detail'),
      visibility: document.visibilityState, online: navigator.onLine,
      viewport: {width: innerWidth, height: innerHeight,
        visual_width: window.visualViewport?.width,
        visual_height: window.visualViewport?.height},
      cache: entry ? {messages: entry.msgs?.length || 0, end: entry.end,
        anchor: entry.anchor, activity: entry.activity?.state || ''} : null,
      dom_messages: nodes.length, terminal: termState,
      header: consoleButtonState(),
    },
    content: {
      composer: $('#cinput')?.value || '',
      messages: nodes.map(node => ({
        id: node.id || '', role: node.dataset?.role || '',
        call_id: node.dataset?.callId || '', classes: node.className,
        text: (node.textContent || '').slice(0, 4000),
      })),
    },
  };
}

let browserSnapshotTimer = 0;
let lastBrowserSnapshot = '';
function scheduleBrowserSnapshot(reason = 'render') {
  if (!capabilities.allows('audit')) return;
  clearTimeout(browserSnapshotTimer);
  browserSnapshotTimer = setTimeout(() => {
    try {
      const snapshot = browserStateSnapshot(reason);
      const signature = JSON.stringify(snapshot);
      if (signature === lastBrowserSnapshot) return;
      lastBrowserSnapshot = signature;
      browserAuditEvent('dom.snapshot', snapshot.data, snapshot.content);
    } catch { /* page can be between detail teardown and rebuild */ }
  }, 100);
}

// ---- 会话标题栏 / 控制台按钮的存在性审计 ----
// 控制台按钮"偶尔消失"一直没抓到现场：快照只看消息，不看标题栏。这里独立记录
// 按钮的三层状态——在不在 DOM、几何上有没有被裁掉/盖住（elementFromPoint）、
// #right 有没有被滚走——任何一层不成立就记一条 console.button.missing，附上
// 标题栏 HTML 和布局数据；恢复时记 console.button.restored。
// 常规状态变化（文案、接管态、灰态、位置）按签名去重记 console.button.state。
function consoleButtonState() {
  const button = $('#a-term');
  const right = $('#right');
  const detail = $('#detail');
  const head = detail?.querySelector(':scope > .dhead');
  const shown = !!right && right.offsetWidth > 0;   // 手机列表页 #right 整个 display:none
  const state = {
    present: !!button, head: !!head, shown,
    detail_children: detail ? [...detail.children].map(
      node => node.id || node.className.split(' ')[0] || node.tagName.toLowerCase()).slice(0, 8) : null,
    right_scroll: right ? [right.scrollLeft, right.scrollTop] : null,
    right_overflow: right ? [right.scrollWidth - right.clientWidth, right.scrollHeight - right.clientHeight] : null,
    tier: layoutTier(), mobile_detail: document.body.classList.contains('mobile-detail'),
  };
  if (button) {
    const style = getComputedStyle(button);
    state.label = button.ariaLabel || '';
    state.on = button.classList.contains('on');
    state.unavailable = button.dataset.unavailable === 'true';
    state.hidden = button.hidden || style.display === 'none' || style.visibility !== 'visible'
      || parseFloat(style.opacity) === 0;
  }
  if (button && shown) {
    const r = button.getBoundingClientRect(), rr = right.getBoundingClientRect();
    const hit = r.width && r.height && document.visibilityState === 'visible'
      ? document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2) : undefined;
    state.rect = [Math.round(r.x), Math.round(r.y), Math.round(r.width), Math.round(r.height)];
    state.in_right = r.width > 0 && r.right <= rr.right + .5 && r.left >= rr.left - .5
      && r.top >= rr.top - .5 && r.bottom <= rr.bottom + .5;
    // 页面不可见时 elementFromPoint 一律 null，不算被遮
    state.hit = hit === undefined ? null : !!hit && button.contains(hit);
    // SVG 元素的 className 是 SVGAnimatedString，只能从 classList 取
    state.hit_target = hit ? (hit.id ? '#' + hit.id : hit.classList?.[0] ? '.' + hit.classList[0]
      : hit.tagName.toLowerCase()) : null;
  }
  // 不在 DOM 一定算丢；在 DOM 但页面可见时被隐藏/裁掉/盖住也算丢
  state.ok = state.present && (!shown || (!state.hidden && state.in_right && state.hit !== false));
  return state;
}

let consoleButtonMissingSince = 0;
let consoleButtonSignature = '';
let consoleButtonTimer = 0;
function auditConsoleButton(reason = '') {
  let state;
  try { state = consoleButtonState(); } catch { return; }
  const content = () => {
    const head = $('#detail > .dhead');
    return {dhead: head ? head.outerHTML.slice(0, 12000) : null,
      detail: $('#detail')?.innerHTML.slice(0, 2000) || null};
  };
  const term = typeof terminal.state === 'undefined' ? null : {
    name: terminal.state.name, uid: terminal.state.uid, mode: terminal.state.mode, views: [...terminal.state.views.keys()],
    visible: !$('#termpane')?.classList.contains('hidden'),
  };
  if (!state.ok) {
    // 丢失期间最多 5 秒记一次，免得 MutationObserver/定时器把库刷爆
    const now = Date.now();
    if (consoleButtonMissingSince && now - consoleButtonMissingSince < 5000) return;
    consoleButtonMissingSince = consoleButtonMissingSince || now;
    browserAuditEvent('console.button.missing', {reason, ...state, selected: selection.sel, agent: selection.agent,
      terminal: term}, content(), {severity: 'error'});
    return;
  }
  if (consoleButtonMissingSince) {
    browserAuditEvent('console.button.restored', {reason, ...state,
      missing_ms: Date.now() - consoleButtonMissingSince, terminal: term}, null, {severity: 'warning'});
    consoleButtonMissingSince = 0;
  }
  const signature = JSON.stringify([state.label, state.on, state.unavailable, state.rect, state.tier,
    state.hit_target, state.right_scroll]);
  if (signature === consoleButtonSignature) return;
  consoleButtonSignature = signature;
  browserAuditEvent('console.button.state', {reason, ...state, terminal: term});
}
function scheduleConsoleButtonAudit(reason = '') {
  clearTimeout(consoleButtonTimer);
  consoleButtonTimer = setTimeout(() => auditConsoleButton(reason), 150);
}
/** #detail 每次换内容都记一笔：谁换的、换完有没有标题栏和控制台按钮。 */
function auditDetailRendered(source, extra = {}) {
  const detail = $('#detail');
  browserAuditEvent('detail.rendered', {
    source, selected: selection.sel, agent: selection.agent, ...extra,
    has_head: !!detail?.querySelector(':scope > .dhead'), has_console_button: !!$('#a-term'),
    children: detail ? [...detail.children].map(
      node => node.id || node.className.split(' ')[0] || node.tagName.toLowerCase()).slice(0, 8) : null,
  });
}

function start() {
  observeLongFrames();
const nativeAlert = window.alert.bind(window);
const nativeConfirm = window.confirm.bind(window);
window.alert = message => {
  const text = String(message ?? '').slice(0, 500);
  browserAuditEvent('dialog.shown', {kind: 'alert', text});
  try { return nativeAlert(message); }
  finally { browserAuditEvent('dialog.closed', {kind: 'alert', result: null}); }
};
window.confirm = message => {
  const text = String(message ?? '').slice(0, 500);
  browserAuditEvent('dialog.shown', {kind: 'confirm', text});
  let result = null;
  try { return (result = nativeConfirm(message)); }
  finally { browserAuditEvent('dialog.closed', {kind: 'confirm', result}); }
};

{
  // #detail 换内容（读取中/失败/正式渲染/新会话页）时重新盯住新的标题栏；标题栏
  // 内部的增删和 class/hidden/style 变化也触发检查。#msgs 的海量变动不在观察范围内。
  const headObserver = new MutationObserver(() => scheduleConsoleButtonAudit('head-mutation'));
  let observedHead = null;
  const watchHead = () => {
    const head = $('#detail > .dhead');
    if (head === observedHead) return;
    headObserver.disconnect();
    observedHead = head;
    if (head) headObserver.observe(head, {childList: true, subtree: true, attributes: true,
      attributeFilter: ['class', 'hidden', 'style', 'aria-label']});
  };
  new MutationObserver(() => { watchHead(); scheduleConsoleButtonAudit('detail-mutation'); })
    .observe($('#detail'), {childList: true});
  watchHead();
  // 裁切/滚走这类不改 DOM 的情况靠 #right 的 scroll 和低频巡检兜底
  $('#right')?.addEventListener('scroll', () => scheduleConsoleButtonAudit('right-scroll'), {passive: true});
  setInterval(() => { if (!document.hidden) auditConsoleButton('periodic'); }, 5000);
  document.addEventListener('visibilitychange', () => {
    if (!document.hidden) scheduleConsoleButtonAudit('visible');
  });
}

queueMicrotask(() => browserAuditEvent('page.loaded', {
  url: location.pathname + location.search, referrer: document.referrer,
  user_agent: navigator.userAgent, language: navigator.language,
  viewport: {width: innerWidth, height: innerHeight}, theme: document.documentElement.dataset.theme,
}));
window.addEventListener('error', event => browserAuditEvent('error', {
  message: event.message, filename: event.filename, line: event.lineno, column: event.colno,
}, null, {severity: 'error'}));
window.addEventListener('unhandledrejection', event => browserAuditEvent('unhandledrejection', {
  reason: String(event.reason?.stack || event.reason || 'unknown'),
}, null, {severity: 'error'}));
window.addEventListener('online', () => browserAuditEvent('network.online'));
window.addEventListener('offline', () => browserAuditEvent('network.offline', {}, null,
  {severity: 'warning'}));
window.addEventListener('pagehide', () => {
  const snapshot = browserStateSnapshot('pagehide');
  browserAuditEvent('page.hidden', snapshot.data, snapshot.content);
  flushBrowserAuditBeacon();
});
document.addEventListener('visibilitychange', () => {
  browserAuditEvent('visibility.changed', {visibility: document.visibilityState});
  if (!document.hidden) scheduleBrowserSnapshot('visible');
});
document.addEventListener('click', event => {
  const target = event.target?.closest?.(
    'button, a, [role="button"], .item, .ghead, summary, label, input, select, textarea, [data-action], [data-term-key], [data-term-modifier], .fold-preview, .turn-toolbar');
  if (!target) return;
  browserAuditEvent('ui.clicked', {
    tag: target.tagName, id: target.id || '', classes: target.className || '',
    title: target.getAttribute('title') || '', uid: target.dataset?.uid || '',
    action: target.dataset?.v || target.dataset?.termKey || target.dataset?.attach || '',
    text: (target.textContent || '').trim().slice(0, 80),
    x: Math.round(event.clientX), y: Math.round(event.clientY),
  });
}, true);
let auditResizeTimer = 0;
window.addEventListener('resize', () => {
  clearTimeout(auditResizeTimer);
  auditResizeTimer = setTimeout(() => {
    browserAuditEvent('viewport.resized', {
      width: innerWidth, height: innerHeight,
      visual_width: window.visualViewport?.width,
      visual_height: window.visualViewport?.height,
    });
    scheduleBrowserSnapshot('resized');
  }, 200);
});
}
  return {longFrameEvent, observeLongFrames, browserStateSnapshot, scheduleBrowserSnapshot, consoleButtonState, auditConsoleButton, scheduleConsoleButtonAudit, auditDetailRendered, start};
}
