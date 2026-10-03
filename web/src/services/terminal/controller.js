import { measureKeyboardClosedLayout, visualKeyboardOpen } from '../shell/viewport';
import { mountTerminalMenu } from './menu';
// @ts-expect-error Original grid facade is JavaScript shared with the standalone page.
import { GridTerm } from '../../../../legacy-web/grid/facade.js';

/** @param {import('./bridge').TerminalBridge} bridge
 * @param {import('../../stores/terminal').TerminalUiState} ui
 * @param {() => void} publish */
export function createTerminalController(bridge, ui, publish) {
const {APP_BASE, HUB_MODE, MOBILE, Nodes, SessionDockCapabilities, SessionDockNetwork, store, SOURCES, appUrl, applyNodeState, nodeOf, newNodeId, sessionTerminalEnabled, sessiondockCli} = bridge.environment;
const {S, ConsoleUI, cache, forkAncestors, forkLeafUid, loadSessions, openSession, paintLive, pendingTmuxSessions, pendingUid, pollLive, sidebarSessions, trimCache, viewKey, deleteSessions} = bridge.sessions;
const {composerDraft, consolePasteFiles, conversationSendEnabled, deleteComposerDraftStorage, followServerDraft, hydrateComposerDraft, migrateComposerDraft, pollComposerInput, recoverComposerDrafts, recoverServerComposerDrafts, renderComposer, renderComposerItems, sendToSession, syncComposerDraftBindings, syncComposerUnloadProtection} = bridge.composer;
const {appAlert, appConfirm, auditDetailRendered, browserAuditEvent, ensureConsolePlaceholder, layoutHeader, renderChips, renderConversationTail, renderMachineSettings, renderPendingSessionAction, renderSide, renderTakeoverBtn, showConsoleToast, showMobileList, showNewSessionStage, showSessionCount, showSessionStopNotice} = bridge.presentation;
const post = bridge.post;
const $ = selector => document.querySelector(selector);
const el = (tag, cls = '', text = '') => { const node = document.createElement(tag); node.className = cls; node.textContent = text; return node; };
'use strict';

// 接管会话: 在服务端把它用 tmux resume 起来, 然后把终端嵌在会话详情底部。
// SSH 例外：PTY 在输入框上方。
// 会话跑在 tmux 里, 所以关掉页面/重启 sessiondock 都不会打断它。
// Hub 的节点侧连接/响应上限是 10 秒；再留出反向代理与浏览器调度余量。
// WebSocket 没有标准的建立超时，必须由页面回收永久 CONNECTING 的尝试。
const TERM_CONNECT_TIMEOUT_MS = 15_000;
const TERM_HEARTBEAT_INTERVAL_MS = 3_000;
const TERM_HEARTBEAT_TIMEOUT_MS = 10_000;
// Claim includes browser/proxy transit plus the Hub's 5 s connect / 10 s read
// waits. A 5 s page deadline can cancel before the node even sees the request.
const TERM_CLAIM_TIMEOUT_MS = 20_000;
// DEC 2026 同步帧在页面这层暂存的上限：超过就先交给 xterm（它自己对 2026 还有
// 1 s 兜底），不让一个没收尾的帧无限占住输出。
const TERM_SYNC_HOLD_MAX = 256 * 1024;
const TERM_SYNC_HOLD_MS = 100;
const TERM_LAYOUT_POLICY_VERSION = 2;
// 每次页面加载独立生成；不写 local/sessionStorage，复制标签页也不会复制归属。
const TERM_PAGE_ID = bridge.pageId;

const T = {
  term: null,      // xterm 实例
  ws: null,
  name: null,      // 当前挂着的 tmux 会话名
  uid: null,       // 对应的 SessionDock 会话
  views: new Map(), // 已打开过且仍存活的 tmux → xterm/WebSocket；切会话只隐藏
  ended: new Map(), // Rust only: explicit host exit, pinned to UID/instance.
  enabled: false,
  listLoaded: false,
  listRequest: null,   // 进行中的 api/term/list 请求；首屏的 live 轮询复用它而不是再发一次
  listLoadedAt: 0,     // 上次列表成功返回的时刻（performance.now）；live 轮询据此跳过刚拉过的重复请求
  listError: '',
  unavailable_reason: '',
  height: store.get('termh', 320),
  mode: store.get('termmode', 'full'), // normal(手动分屏) | collapsed(对话：PTY+输入) | full(纯终端)
  shiftSelect: false,                       // 手机 Shift：锁定本地拖动选字，不发送到 CLI
  altArmed: false,                          // 手机 Alt：只修饰下一次输入
  ctrlArmed: false,                         // 手机 Ctrl / 桌面右 Ctrl：只修饰下一次输入
  sources: {},
  home: '',
  pending: [],
  pendingModes: new Map(), // 临时会话名 → 打开前的终端布局；退出未落盘时恢复
  resolving: new Set(),
  discarding: new Set(),
  resolveControllers: new Map(),
  openViews: new Map(store.get('termviews', [])), // tmux 名 → {mode, height}
};
// 旧版把 normal 当默认布局，无法区分“系统默认分屏”和“用户手动分屏”。
// 升级时只迁移一次；此后 normal 只会由拖动分界线产生并照常按会话保存。
if (+store.get('termLayoutPolicyVersion', 0) < TERM_LAYOUT_POLICY_VERSION) {
  if (T.mode === 'normal') T.mode = 'full';
  T.openViews = new Map([...T.openViews].map(([name, layout]) => [
    name, layout?.mode === 'normal' ? { ...layout, mode: 'full' } : layout,
  ]));
  store.set('termmode', T.mode);
  store.set('termviews', [...T.openViews]);
  store.set('termLayoutPolicyVersion', TERM_LAYOUT_POLICY_VERSION);
}
const TERM_FONT_SAMPLE = 'MW0il中文，。！？（）【】';
let resolvedTermFont = '';
let resolvedTermFontKey = '';
let termFontResolveEpoch = 0;

function termTheme() {
  const css = getComputedStyle(document.querySelector('#xterm') || document.documentElement);
  const read = name => css.getPropertyValue(name).trim();
  return {
    background: read('--terminal-bg'), foreground: read('--terminal-fg'),
    cursor: read('--terminal-cursor'), selectionBackground: read('--terminal-selection'),
    black: read('--terminal-black'), red: read('--terminal-red'),
    green: read('--terminal-green'), yellow: read('--terminal-yellow'),
    blue: read('--terminal-blue'), magenta: read('--terminal-magenta'),
    cyan: read('--terminal-cyan'), white: read('--terminal-white'),
    brightBlack: read('--terminal-bright-black'), brightRed: read('--terminal-bright-red'),
    brightGreen: read('--terminal-bright-green'), brightYellow: read('--terminal-bright-yellow'),
    brightBlue: read('--terminal-bright-blue'), brightMagenta: read('--terminal-bright-magenta'),
    brightCyan: read('--terminal-bright-cyan'), brightWhite: read('--terminal-bright-white'),
  };
}

function configuredTermFont() {
  return getComputedStyle(document.documentElement).getPropertyValue('--terminal-font').trim();
}

function termFont() {
  return resolvedTermFont || configuredTermFont();
}

function termFontSize() {
  const value = parseFloat(getComputedStyle(document.documentElement)
    .getPropertyValue('--terminal-font-size'));
  return Number.isFinite(value) ? value : 14.04;
}

function terminalFontGridRatio(family, size) {
  const context = document.createElement('canvas').getContext('2d');
  if (!context) return 0;
  context.font = `400 ${size}px ${family}`;
  const latin = context.measureText('0').width;
  const cjk = context.measureText('中').width;
  return latin > 0 ? cjk / latin : 0;
}

/** Resolve one font face whose CJK glyph is exactly two Latin cells wide.
 * Ubuntu keeps its original glyph size: xterm reserves two cells for CJK and
 * rescaleOverlappingGlyphs prevents wide outlines from crossing cell bounds.
 * Other mixed stacks still prefer a locally available exact 1:2 font. */
async function prepareTerminalFont() {
  const configured = configuredTermFont();
  const size = termFontSize();
  const key = `${size}\n${configured}`;
  if (resolvedTermFont && resolvedTermFontKey === key) return resolvedTermFont;
  // 设置项刚切换时先停止返回旧字体；异步探测期间至少立即使用新选择。
  if (resolvedTermFontKey !== key) {
    resolvedTermFont = '';
    resolvedTermFontKey = '';
  }
  const epoch = ++termFontResolveEpoch;
  let resolved = configured;
  try {
    await document.fonts?.load(`${size}px ${configured}`, TERM_FONT_SAMPLE);
    const configuredRatio = terminalFontGridRatio(configured, size);
    const keepUbuntuGlyphs = configured.includes('"SessionDock Ubuntu Sans Mono"');
    if (!keepUbuntuGlyphs && Math.abs(configuredRatio - 2) > .025) {
      const grid = '"SessionDock CJK Mono Grid"';
      const faces = await document.fonts?.load(`${size}px ${grid}`, TERM_FONT_SAMPLE);
      const ratio = faces?.length ? terminalFontGridRatio(grid, size) : 0;
      if (Math.abs(ratio - 2) <= .025) resolved = `${grid}, ${configured}`;
    }
  } catch { /* 本机没有 Noto/Sarasa 时保留原字体回退 */ }
  if (epoch === termFontResolveEpoch) {
    resolvedTermFont = resolved;
    resolvedTermFontKey = key;
  }
  return resolved;
}

function hexToRgb(hex) {
  const value = (hex || '').trim().replace('#', '');
  if (value.length === 3) {
    return [...value].map(ch => parseInt(ch + ch, 16));
  }
  if (value.length !== 6) return null;
  return [0, 2, 4].map(i => parseInt(value.slice(i, i + 2), 16));
}

function hslLightness(r, g, b) {
  r /= 255; g /= 255; b /= 255;
  return (Math.max(r, g, b) + Math.min(r, g, b)) / 2;
}

// 保持色相、反射亮度；轻微曲线把中间色拉回 50%，避免彩色文字过艳。
function reflectedLightRgb(r, g, b, background = false) {
  r /= 255; g /= 255; b /= 255;
  const hi = Math.max(r, g, b), lo = Math.min(r, g, b);
  const sourceLight = (hi + lo) / 2;
  let h = 0, s = 0;
  if (hi !== lo) {
    const d = hi - lo;
    s = d / (1 - Math.abs(2 * sourceLight - 1));
    if (hi === r) h = ((g - b) / d) % 6;
    else if (hi === g) h = (b - r) / d + 2;
    else h = (r - g) / d + 4;
    h = (h * 60 + 360) % 360;
  }
  const reflected = 1 - sourceLight;
  const sign = Math.sign(reflected - .5);
  let l = .5 + sign * .5 * Math.pow(Math.abs(reflected - .5) / .5, 1.35);
  if (background) l = Math.min(l, .96);   // 显式黑底随亮色方案恢复为接近白色
  const c = (1 - Math.abs(2 * l - 1)) * s;
  const x = c * (1 - Math.abs((h / 60) % 2 - 1));
  const m = l - c / 2;
  let rr = 0, gg = 0, bb = 0;
  if (h < 60) [rr, gg, bb] = [c, x, 0];
  else if (h < 120) [rr, gg, bb] = [x, c, 0];
  else if (h < 180) [rr, gg, bb] = [0, c, x];
  else if (h < 240) [rr, gg, bb] = [0, x, c];
  else if (h < 300) [rr, gg, bb] = [x, 0, c];
  else [rr, gg, bb] = [c, 0, x];
  return [rr, gg, bb].map(v => Math.max(0, Math.min(255, Math.round((v + m) * 255))));
}

function indexedTerminalRgb(n) {
  if (n >= 0 && n <= 15) {
    const theme = termTheme();
    const keys = [
      'black', 'red', 'green', 'yellow', 'blue', 'magenta', 'cyan', 'white',
      'brightBlack', 'brightRed', 'brightGreen', 'brightYellow',
      'brightBlue', 'brightMagenta', 'brightCyan', 'brightWhite',
    ];
    return hexToRgb(theme[keys[n]]);
  }
  if (n >= 232 && n <= 255) {
    const v = 8 + (n - 232) * 10;
    return [v, v, v];
  }
  if (n < 16 || n > 231) return null;
  const steps = [0, 95, 135, 175, 215, 255];
  n -= 16;
  return [steps[Math.floor(n / 36)], steps[Math.floor(n / 6) % 6], steps[n % 6]];
}

function adaptRgbForLight(r, g, b, background) {
  r = Math.max(0, Math.min(255, +r));
  g = Math.max(0, Math.min(255, +g));
  b = Math.max(0, Math.min(255, +b));
  const light = hslLightness(r, g, b);
  // 已是浅底/深字的真彩色（Codex diff、浅色 prompt）保持原样；
  // 只有和亮色页面冲突的深底/浅字才反射亮度。
  if (background ? light >= .5 : light < .5) return [r, g, b];
  return reflectedLightRgb(r, g, b, background);
}

function adaptColonRgb(token) {
  const match = token.match(/^(38|48):2(?::\d*)?:(\d{1,3}):(\d{1,3}):(\d{1,3})$/);
  if (!match) return token;
  const rgb = adaptRgbForLight(+match[2], +match[3], +match[4], match[1] === '48');
  return `${match[1]};2;${rgb.join(';')}`;
}

function adaptSgrBody(body) {
  const tokens = body.split(';');
  const out = [];
  for (let i = 0; i < tokens.length; i++) {
    const raw = tokens[i];
    if (/^(38|48):2/.test(raw)) {
      out.push(adaptColonRgb(raw));
      continue;
    }
    const code = raw === '' ? 0 : Number(raw);
    if ((code === 38 || code === 48) && tokens[i + 1] === '2' && i + 4 < tokens.length) {
      const rgb = adaptRgbForLight(tokens[i + 2], tokens[i + 3], tokens[i + 4], code === 48);
      out.push(String(code), '2', ...rgb.map(String));
      i += 4;
      continue;
    }
    if ((code === 38 || code === 48) && tokens[i + 1] === '5' && i + 2 < tokens.length) {
      const source = indexedTerminalRgb(+tokens[i + 2]);
      if (!source) {
        out.push(String(code), '5', tokens[i + 2]);
      } else {
        const rgb = adaptRgbForLight(...source, code === 48);
        out.push(String(code), '2', ...rgb.map(String));
      }
      i += 2;
      continue;
    }
    let palette = -1, background = false;
    if (code >= 40 && code <= 47) { palette = code - 40; background = true; }
    else if (code >= 100 && code <= 107) { palette = code - 100 + 8; background = true; }
    else if (code >= 30 && code <= 37) palette = code - 30;
    else if (code >= 90 && code <= 97) palette = code - 90 + 8;
    if (palette >= 0) {
      const source = indexedTerminalRgb(palette);
      if (source) {
        const light = hslLightness(...source);
        const clashes = background ? light < .5 : light >= .5;
        if (clashes) {
          const rgb = adaptRgbForLight(...source, background);
          out.push(background ? '48' : '38', '2', ...rgb.map(String));
          continue;
        }
      }
    }
    out.push(raw);
  }
  return out.join(';');
}

function stripOscColorSets(s) {
  // 丢掉 CLI 把默认前景/背景/光标改成黑底的 OSC。查询（11;?）仍交给 xterm；
  // 回包在 stripOscColorReports 里丢掉，不写回 PTY。
  return s.replace(/\x1b\](?:10|11|12|104|110|111|112);(?!\?)[^\x07\x1b]*(?:\x07|\x1b\\)/g, '');
}

// xterm 会回答 OSC 10/11/12/4 查询，答案走 term.onData，看起来像键盘输入。
// 把回包改写成暗色 palettes 再写回 PTY，会让 gh/survey 这类在查询后立刻
// 进 raw 读键的 CLI 把 ESC ] 当成非法按键（leftover 11;rgb:0000/0000/0000）。
// 丢掉回包：CLI 超时后沿用默认暗色 TUI，亮色页面只在浏览器反色，
// 也不把页面底色告诉 Codex/Claude/Grok。
function stripOscColorReports(s) {
  if (!s || !s.includes('\x1b]')) return s;
  s = s.replace(
    /\x1b\](?:10|11|12);(?:rgb:[^\\\x07]*|#[0-9a-fA-F]+)(?:\x07|\x1b\\)/g, '');
  return s.replace(
    /\x1b\]4;\d+;(?:rgb:[^\\\x07]*|#[0-9a-fA-F]+)(?:\x07|\x1b\\)/g, '');
}

function lightTerminalAnsi(s) {
  return s.replace(/\x1b\[([0-9;:]*)m/g, (_, body) => `\x1b[${adaptSgrBody(body)}m`);
}

function terminalColorChunk(view, s) {
  s = (view.ansiTail || '') + s;
  view.ansiTail = '';
  // PTY/WebSocket 可能恰好在 CSI/OSC 中间断包，留下不完整尾巴等下一块再处理。
  const tail = s.match(/\x1b(?:\[[?0-9;:]*|\][^\x07\x1b]*)$/)?.[0] || '';
  if (tail) {
    view.ansiTail = tail;
    s = s.slice(0, -tail.length);
  }
  s = stripOscColorSets(s);
  if (document.documentElement.dataset.theme !== 'light') return s;
  return lightTerminalAnsi(s);
}

async function refreshTerminalPreferences(redraw = false) {
  terminalFontReady = prepareTerminalFont();
  try { await terminalFontReady; } catch {}
  const views = T.views ? T.views.values() : (T.term ? [{ term: T.term }] : []);
  const liveNames = new Set([...(T.list || []), ...(T.pending || [])]
    .map(item => item.name));
  const redrawNames = [];
  for (const view of views) {
    view.term.options.fontFamily = termFont();
    view.term.options.theme = termTheme();
    // 已退出的临时会话可能在下一次列表轮询前仍留有一个 xterm 视图。
    // 配色切换只重连目前确实存在的 tmux，不能拿陈旧视图去 claim 404。
    const currentConnected = T.name === view.name && view.ws?.readyState === WebSocket.OPEN;
    if (redraw && view.name && (liveNames.has(view.name) || currentConnected)) {
      redrawNames.push(view.name);
    }
  }
  for (const name of redrawNames) void attachTerm(name, true);
  setTimeout(() => { fitTerm(); }, 0);
}

// xterm 先测量网页字体再创建 DOM 行，避免按回退字体计算出错误的字符宽度。
let terminalFontReady = prepareTerminalFont();
addEventListener('resize', () => { layoutTermPane(); fitTerm(); });

let termListRequestSeq = 0;
function terminalListUncertain(uid) {
  return !T.listLoaded || !!T.listError
    || (HUB_MODE && !!Nodes.errors.get('term')?.some(error => error.node_id === nodeOf(uid)));
}

function mergeUnavailableTermRows(rows, previous, errors) {
  if (SessionDockCapabilities.config.backend !== 'rust' || !HUB_MODE) return rows;
  const failed = new Set((errors || []).map(error => error.node_id));
  const retained = (previous || []).filter(row => failed.has(row.node_id || nodeOf(pendingUid(row.name))));
  const names = new Set(retained.map(row => row.name));
  // A partial hub response cannot revoke a newer create receipt or open view.
  // Keep its identity, but mark the observation stale until that node answers.
  return [...rows.filter(row => !names.has(row.name)), ...retained.map(row => ({...row, stale:true}))];
}

function loadTermList() {
  if (SessionDockNetwork.paused) return Promise.resolve();
  const request = fetchTermList().finally(() => { if (T.listRequest === request) T.listRequest = null; });
  T.listRequest = request;
  return request;
}
async function fetchTermList() {
  const requestSeq = ++termListRequestSeq;
  const openEpoch = termOpenEpoch;
  const fingerprint = () => [
    ...(T.list || []).map(x => `${x.name}\t${x.cwd}` + (SessionDockCapabilities.config.backend === 'rust' ? `\t${x.uid}\t${x.instance_id}\t${x.current_uid || ''}\t${x.frozen}` : '')),
    ...(T.pending || []).map(x => `pending\t${x.name}\t${x.cwd}` + (SessionDockCapabilities.config.backend === 'rust' ? `\t${x.record_id}\t${x.instance_id}\t${x.state}\t${pendingPhase(x)}` : '')),
  ].join('\n');
  const before = fingerprint();
  let loaded = false;
  let data = null;
  let failure = '';
  let transient = false;   // Rust：网络错误 / 5xx / 429 只影响这一轮，不清空控制台状态
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 12000);
  try {
    const response = await fetch(appUrl('api/term/list'), {signal:controller.signal});
    transient = response.status === 429 || response.status >= 500;
    data = await response.json();
    if (!response.ok || data.error) throw new Error(
      `终端列表请求失败（HTTP ${response.status}）：${data.error || response.statusText}`);
    loaded = true;
    transient = false;
  } catch (error) {
    if (SessionDockNetwork.paused) return;
    failure = error.message || String(error);
    if (error?.name === 'TypeError' || controller.signal.aborted) transient = true;
  } finally { clearTimeout(timer); }
  // 只允许最后发出的请求改状态；否则慢响应会覆盖更新的 tmux 列表。
  if (requestSeq !== termListRequestSeq) return;
  // 请求在终端打开/切换之前发出时，它的“没有该视图”结论已经过期。丢弃
  // 整份结果并立刻重取，不能先销毁刚打开的常驻 xterm 再补回来。
  if (openEpoch !== termOpenEpoch) {
    void loadTermList();
    return;
  }
  T.listLoaded = true;
  T.listError = loaded ? '' : `无法读取控制台状态：${failure}`;
  if (loaded) {
    T.listLoadedAt = performance.now();
    applyNodeState(data, 'term');
    T.enabled = !!data.enabled;
    T.unavailable_reason = data.unavailable_reason || '';
    T.list = mergeUnavailableTermRows(data.sessions || [], T.list, data.errors);
    T.sources = data.sources || {};
    T.resume_sources = data.resume_sources || {};
    T.home = data.home || '';
    const hadSelected = String(S.sel || '').startsWith('tmux:')
      && (T.pending || []).some(row => pendingUid(row.name) === S.sel);
    T.pending = mergeUnavailableTermRows(data.pending || [], T.pending, data.errors);
    if (hadSelected && !T.pending.some(row => pendingUid(row.name) === S.sel)
        && !T.discarding.has(String(S.sel).slice(5))
        && !(typeof pendingTmuxSessions === 'function' && pendingTmuxSessions().some(row => row.uid === S.sel)))
      pendingSelectionGone(String(S.sel).slice(5));
    if (typeof recoverServerComposerDrafts === 'function') void recoverServerComposerDrafts();
    if (typeof syncComposerDraftBindings === 'function') syncComposerDraftBindings();
    if (SessionDockCapabilities.config.backend === 'rust') {
      for (const [uid, ended] of T.ended) {
        const replacement = T.list.find(row => row.uid === uid && row.instance_id !== ended.instanceId);
        if (replacement) {
          T.ended.delete(uid);
          if (ConsoleUI.errors.get(uid) === ended.reason) ConsoleUI.errors.delete(uid);
        }
      }
    }
  } else if (SessionDockCapabilities.config.backend === 'rust' && transient) {
    // 瞬时失败：保留上一轮的 enabled/sources/list/pending，“+”与接管按钮不消失；
    // 下一轮轮询自然恢复。只有明确的 4xx 才把状态清空。
  } else {
    T.enabled = false;
    T.list = [];
    T.sources = {};
    T.resume_sources = {};
    T.pending = [];
  }
  // 设置面板开着时，后端清单要跟着刷新，否则显示的是上一轮的状态。
  if (typeof renderMachineSettings === 'function'
      && document.querySelector('#settings-dialog')?.open
      && !document.querySelector('#settings-machines')?.hidden) renderMachineSettings();
  if (loaded) {
    const valid = new Set([...T.list, ...T.pending].map(x => x.name));
    const kept = new Map([...T.openViews].filter(([name, saved]) => {
      if (!valid.has(name)) return false;
      if (SessionDockCapabilities.config.backend !== 'rust') return true;
      const row = (saved.record_id ? T.pending : T.list).find(row => row.name === name);
      return !!row && saved.uid === (row.uid || (row.record_id && pendingUid(row.name)))
        && saved.instance_id === row.instance_id && (!row.record_id || saved.record_id === row.record_id);
    }));
    if (kept.size !== T.openViews.size) {
      T.openViews = kept;
      store.set('termviews', [...kept]);
    }
    for (const name of T.views.keys()) {
      const current = [...T.list, ...T.pending].find(row => row.name === name);
      const replaced = SessionDockCapabilities.config.backend === 'rust'
        && current
        && T.views.get(name)?.instanceId
        && T.views.get(name).instanceId !== current.instance_id;
      const view = T.views.get(name);
      const keepFinalOutput = (view?.ended || view?.retired)
        && ((T.name === name && !!ui.visible)
          || (view?.keepOutput && termBindingServes(view.bindingUid, S.sel)));
      if (replaced || (!valid.has(name) && !keepFinalOutput)) disposeTermView(name);
    }
    // tmux 结束后，对应的消息缓存才重新回到普通 LRU 容量池。
    if (typeof trimCache === 'function') trimCache();
    // Codex 回退会创建新分支 UUID，但原生进程与 tmux pane 都不变。
    // term/list 已经把 pane 映射到当前叶子；若本页还选中旧叶子，
    // 必须连同草稿和终端归属一起跟进，不能继续向已消失的 uid 请求接管。
    await rebindSelectedTermSession();
    const view = currentTermViewObject();
    if (view && !view.replay && (view.ended || view.keepOutput)
        && !!ui.visible
        && typeof startShellRecordingReplay === 'function')
      startShellRecordingReplay(view, view.bindingUid || T.uid);
  }
  const create = $('#new-session');
  const createEnabled = T.enabled && SessionDockCapabilities.allows('terminal_create');
  if (create && create.classList.contains('hidden') === createEnabled) {
    create.classList.toggle('hidden', !createEnabled);
    // 新建按钮出现/消失改变顶栏右侧占宽，放不放得下要重新量
    if (typeof layoutHeader === 'function') layoutHeader();
  }
  renderTakeoverBtn();
  const after = fingerprint();
  if (after !== before && S.sig && typeof renderSide === 'function') {
    const side = $('#side'), top = side?.scrollTop || 0;
    renderChips();
    if (!S.results) showSessionCount(sidebarSessions().length);
    renderSide();
    if (side) side.scrollTop = top;
    paintLive();
  }
  // 刷新页面后仍从持久化 meta 恢复关联轮询；Set 防止重复启动。
  for (const pending of pendingTmuxSessions()) if (!pending.stale) resolveNewSession(pending);
  // Declared launches leave the sidebar once their native record exists;
  // the selected pending page still has to follow that association.
  if (SessionDockCapabilities.config.backend === 'rust')
    for (const row of T.pending) if (row.declared_sid || row.binding?.state === 'confirmed') resolveNewSession(row);
  restoreTermPane(S.sel, S.agent);
  if (String(S.sel || '').startsWith('tmux:')) refreshPendingStage(String(S.sel).slice(5));
}

function sessionTermMeta(uid) {
  return S.sessions.find(x => x.uid === uid)
    || (typeof cache !== 'undefined' ? cache.get(viewKey(uid))?.meta : null)
    || null;
}

/** Rust 下 pane 与 view 绑定的是接管/启动时核验的 uid，Codex 回退后不变；
 *  同一进程改写新分支后，分支叶子沿 forked_from_id 追溯到被绑定的祖先仍算同一
 *  控制台。 */
function termBindingServes(boundUid, uid) {
  if (!boundUid || !uid) return false;
  const current = (T.list || []).filter(row => row.uid === boundUid && row.current_uid);
  if (current.length === 1) return current[0].current_uid === uid;
  if (boundUid === uid) return true;
  if (typeof forkAncestors !== 'function') return false;
  const session = sessionTermMeta(uid);
  return !!session && forkAncestors(session).some(({row}) => row?.uid === boundUid);
}

/** 返回会话所在的稳定 tmux pane 以及 pane 当前对应的 uid。
 *
 * 默认只接受 pane 的精确 uid 归属。Codex 回退后，同一个稳定 pane 会改绑到
 * 新叶子；只有负责跟进回退或用户明确切换终端的调用方才允许追随这个替代 uid。
 * Rust 后端的 pane 行始终写接管时绑定的 uid，叶子由列表的 fork 图推出。
 */
function linkedTermSession(uid, { followReplacement = false } = {}) {
  const panes = [...(T.list || []), ...(T.pending || [])];
  if (SessionDockCapabilities.config.backend === 'rust') {
    const current = panes.filter(pane => pane.instance_id && pane.current_uid === uid);
    if (current.length === 1) return {name: current[0].name, uid};
    if (current.length > 1) return null;
    const moved = panes.filter(pane => pane.instance_id && pane.uid === uid
      && pane.current_uid && pane.current_uid !== uid);
    // Only a proven fork may carry the old draft along; /new is unrelated.
    if (moved.length) {
      const next = moved.length === 1 ? sessionTermMeta(moved[0].current_uid) : null;
      return followReplacement && next && typeof forkAncestors === 'function'
        && forkAncestors(next).some(({row}) => row?.uid === uid)
        ? {name: moved[0].name, uid: moved[0].current_uid} : null;
    }
    const exactFor = target => panes.filter(pane => pane.instance_id && (pane.uid === target
      || (pane.record_id && pane.launch_id && !pane.stale && pendingUid(pane.name) === target)));
    const leafOf = target => (typeof forkLeafUid === 'function' ? forkLeafUid(target) : target);
    const exact = exactFor(uid);
    if (exact.length === 1) {
      const leaf = leafOf(uid);
      return leaf === uid || followReplacement ? {name: exact[0].name, uid: leaf} : null;
    }
    if (exact.length || typeof forkAncestors !== 'function') return null;
    // 回退分支没有自己的 pane：最近一个仍有 pane 的祖先就是它的控制台，前提是
    // 这条分支正是那个 pane 当前写入的叶子。
    const session = sessionTermMeta(uid);
    for (const {row} of session ? forkAncestors(session) : []) {
      if (!row) break;
      const inherited = exactFor(row.uid);
      if (!inherited.length) continue;
      if (inherited.length !== 1) return null;
      const leaf = leafOf(row.uid);
      return leaf === uid || followReplacement ? {name: inherited[0].name, uid: leaf} : null;
    }
    return null;
  }
  const linked = (pane, name = pane?.name) => {
    if (!pane || (!followReplacement && pane.uid && pane.uid !== uid)) return null;
    return { name, uid: pane.uid || uid };
  };
  if (String(uid || '').startsWith('tmux:')) {
    const name = String(uid).slice(5);
    const pane = panes.find(x => x.name === name);
    return pane ? { name, uid: pane.uid || uid } : null;
  }
  const direct = panes.find(x => x.uid === uid);
  if (direct) return { name: direct.name, uid: direct.uid || uid };

  // 终端已经在本页打开时，pane 名是跨分支的稳定身份。
  // 服务端映射出的 pane.uid 才是当前原生叶子。
  if (T.uid === uid && T.name) {
    const active = panes.find(x => x.name === T.name);
    const result = linked(active);
    if (result) return result;
  }

  const session = sessionTermMeta(uid);
  if (!session) return null;
  // Codex 分支的 sid 会变，而接管时的 tmux 名由根会话 sid 生成。
  // 优先保留普通会话的叶子名，再用 root_sid 追溯回同一 pane。
  const ids = [...new Set([session.sid, session.root_sid].filter(Boolean))];
  for (const sid of ids) {
    const name = (session.node_id ? session.node_id + '~' : '') + `sessiondock-${session.source}-${String(sid).slice(0, 8)}`;
    const pane = panes.find(x => x.name === name);
    const result = linked(pane, name);
    if (result) return result;
  }
  return null;
}

/** 某个会话是否已经被接管 (存在对应的 tmux 会话)。 */
function takenOver(uid) {
  return linkedTermSession(uid)?.name || null;
}

/** Rust `outbox`: the reliable-send routes act under this page's
 *  own instance lease when the console is open here, so sending from the
 *  composer never conflicts with our own console. Without a lease the server
 *  claims for itself and reports any other page's lease as an ownership
 *  error. Undeclared capabilities send nothing extra. */
function termSendLease(name) {
  if (SessionDockCapabilities.config.backend !== 'rust'
      || SessionDockCapabilities.config.conversation_send!==true) return {};
  const lease = T.views.get(name)?.inputLease;
  if (!lease?.token || !lease?.instance_id) return {};
  const out = { page: TERM_PAGE_ID, token: lease.token, instance_id: lease.instance_id };
  if (lease.launch_id) out.launch_id = lease.launch_id;
  return { lease: out };
}

/** The pane's pinned identity for a page that holds no console lease here:
 *  the native `uid`+`instance_id` or the launch triple, exactly what attach
 *  would claim. The server then writes under its ordinary-claimant rule
 *  (refused while any page or server send holds the lease) instead of the
 *  unauthenticated `send-keys`. */
function termRowBinding(name, uid) {
  const pending = String(uid || '').startsWith('tmux:');
  const row = (pending ? (T.pending || []) : (T.list || [])).find(row => row.name === name);
  if (!row?.instance_id) return null;
  if (row.record_id && row.launch_id && !row.stale) {
    return { record_id: row.record_id, launch_id: row.launch_id, instance_id: row.instance_id };
  }
  return row.uid ? { uid: row.uid, instance_id: row.instance_id } : null;
}

/** Rust `terminal_input`: raw HTTP text/keys under this page's exact terminal
 *  lease, or with an empty token plus the pane's pinned identity when the
 *  console is not open on this page (the composer's Esc and question cards,
 *  Grok text). Only the legacy HTTP shapes change (named keys, a bracketed
 *  `paste`, and raw text with `enter:false`); a Claude/Codex text submit is
 *  the reliable-send composer (see `termSendLease`), so a bare `text` returns
 *  null. Undeclared capabilities keep the body. */
function termInputBody(name, body) {
  if (SessionDockCapabilities.config.backend !== 'rust'
      || !SessionDockCapabilities.allows('terminal_input')) return body;
  const lease = T.views.get(name)?.inputLease;
  const identity = lease || termRowBinding(name, body.uid);
  const out = { name, page: TERM_PAGE_ID, token: lease?.token || '' };
  for (const key of ['uid', 'instance_id', 'record_id', 'launch_id']) {
    if (identity?.[key]) out[key] = identity[key];
  }
  if (Array.isArray(body.keys)) out.keys = body.keys;
  else if (typeof body.paste === 'string') out.paste = body.paste;
  else if (body.enter === false && typeof body.text === 'string') out.data = body.text;
  else return null;
  return out;
}

function adoptLinkedTermSession(fromUid, linked, reason) {
  const toUid = linked?.uid;
  if (!toUid || toUid === fromUid || String(toUid).startsWith('tmux:')) return fromUid;
  browserAuditEvent?.('terminal.session_rebound', {
    name: linked.name, from_uid: fromUid, to_uid: toUid, reason,
  }, null, { uid: toUid });
  migrateComposerDraft(fromUid, toUid);
  T.uid = toUid;
  return toUid;
}

/** tmux 列表已指向新分支时，原子跟进当前详情与输入状态。 */
async function rebindSelectedTermSession() {
  const fromUid = T.uid;
  if (!fromUid || S.sel !== fromUid || S.agent
      || String(fromUid).startsWith('tmux:')) return false;
  const linked = linkedTermSession(fromUid, { followReplacement: true });
  if (!linked?.uid || linked.uid === fromUid) return false;
  const toUid = adoptLinkedTermSession(fromUid, linked, 'term-list');
  await openSession(toUid);
  return true;
}

/** 顶栏切换前先跟进 pane 的当前分支，然后再执行原本的对话/终端切换。 */
async function toggleLinkedTermSession(uid) {
  const linked = linkedTermSession(uid, { followReplacement: true });
  if (!linked) return false;
  const toUid = adoptLinkedTermSession(uid, linked, 'user-toggle');
  if (toUid !== uid && S.sel === uid && !S.agent) {
    await openSession(toUid);
    // 读取新分支期间用户可能已经切到别处，不再抢回终端。
    if (S.sel !== toUid || S.agent) return true;
  } else {
    T.uid = toUid;
  }
  await toggleTermPane(linked.name);
  return true;
}

// ---------------------------------------------------------------- 接管
async function takeover(uid, btn) {
  const setBtn = (t, dis) => {
    if (btn) { btn.title = btn.ariaLabel = t; btn.setAttribute('aria-busy', String(dis)); }
  };
  setBtn('接管中…', true);
  try {
    // Rust: takeover is an idempotent, server-resolved `resume` receipt; the
    // request ID keeps a retried click from starting a second CLI.
    const rustResume = SessionDockCapabilities.config.backend === 'rust'
      ? {request_id: globalThis.crypto?.randomUUID?.() || `${Date.now()}-${Math.random().toString(36).slice(2)}`} : {};
    let d = await post('api/term/takeover', { uid, cols: 120, rows: termRows(), ...rustResume });
    if (d.needs_confirm) {
      const n = (d.pids || []).length;
      const ok = await appConfirm(
        `这个会话正在运行中（${n} 个进程），而且不在 tmux 里，无法直接接入。\n\n`
        + `接管会先结束正在运行的实例，再用 tmux 重新打开它。\n`
        + `未保存的输入会丢失，已完成的对话不受影响。\n\n继续吗？`);
      if (!ok) return;
      setBtn('结束旧实例…', true);
      d = await post('api/term/takeover', { uid, force: true, cols: 120, rows: termRows() });
    }
    if (d.error) {
      ConsoleUI.errors.set(uid, d.error);
      return appAlert('打开控制台失败：' + d.error);
    }
    await loadTermList();
    // Rust: the receipt is ready, but the fresh instance reaches the console
    // list only once its guarded observation matches the session row; wait
    // for that (bounded) instead of attaching against a stale list.
    for (let attempt = 0; SessionDockCapabilities.config.backend === 'rust' && attempt < 12
         && !(T.list || []).some(row => row.name === d.name && row.instance_id === d.instance_id); attempt++) {
      await new Promise(resolve => setTimeout(resolve, 500));
      await loadTermList();
    }
    T.uid = uid;
    S.live.add(uid);
    S.liveTmux.add(uid);
    paintLive();
    ConsoleUI.errors.delete(uid);
    await openTermPane(d.name);
  } finally {
    setBtn('接管会话', false);
    renderTakeoverBtn();
    renderComposer();
  }
}


const WORKER_STATUS_TEXT = {
  starting: '处理会话正在启动', injecting: '正在注入缺陷报告提示词', submitted: '已发送',
  submitted_unconfirmed: '提示词已粘贴，但未能确认提交', failed: '未发送，输入已保留',
};
function workerStatusMessage(info) {
  if (info?.kind !== 'bug-report' || !info.worker_status) return '';
  const text = WORKER_STATUS_TEXT[info.worker_status] || info.worker_status;
  return info.worker_error ? `${text}：${info.worker_error}` : text;
}

/** 启动型行（SSH 与代理的 receipt）唯一的状态来源。头部按钮、侧栏副标题、
 *  右键菜单和等待页文案都从这里取值，不各自拿 state/running 推断。
 *  本页刚观察到的宿主退出（T.ended，按实例）先于服务端落账生效，列表轮询
 *  回来一份仍写着 running 的旧行也不会把按钮翻回"停止"。 */
function pendingPhase(row) {
  if (!row) return 'gone';
  const ended = typeof T !== 'undefined' && T.ended?.get?.(`tmux:${row.name}`);
  if (ended && (!row.instance_id || ended.instanceId === row.instance_id)) return 'exited';
  if (row.state === 'exited') return 'exited';
  if (row.state === 'failed') return 'failed';
  if (terminalListUncertain(pendingUid(row.name))) return 'uncertain';
  if (row.state === 'cancel_requested') return 'stopping';
  if (row.state === 'uncertain') return 'uncertain';
  if (row.state === 'prepared' || row.state === 'starting') return 'starting';
  if (row.running === false || row.stale) return 'stopping';
  return 'running';
}

/** Sidebar meta text of a Rust pending row (other rows keep "等待首条消息"). */
function pendingStateLabel(s) {
  if (SessionDockCapabilities.config.backend !== 'rust' || !s.record_id) return '等待首条消息';
  if (s.kind === 'bug-report' && s.worker_status && s.worker_status !== 'starting'
      && pendingPhase(s) === 'running')
    return WORKER_STATUS_TEXT[s.worker_status] || s.worker_status;
  switch (pendingPhase(s)) {
    case 'exited': return '已结束';
    case 'failed': return '启动失败';
    case 'stopping': return '正在停止';
    case 'uncertain': return '运行状态不确定';
    case 'starting': return '正在启动';
    default: return s.source === 'shell' ? '交互式终端'
      : sessiondockCli(s.source)?.nativeHistory === false ? '在控制台查看回复' : '等待首条消息';
  }
}

// 等待页只说用户看得懂的事：启动、结束、停止、还没找到记录。关联方法、
// 证据和 uid 不出现在这里；后台每 1.5 s 自动重试关联，页面无需重试按钮。
const PENDING_RECORD_GRACE_MS = 60_000;
const pendingFirstInput = new Map();

function pendingStageMessage(info) {
  const worker = workerStatusMessage(info);
  if (worker) return worker;
  switch (pendingPhase(info)) {
    case 'starting': return '正在启动…';
    case 'exited': return info.source === 'shell' && !info.recording?.id ? '会话已结束，没有留下录制。' : '会话已结束。';
    case 'failed': return '启动失败。';
    case 'stopping': return '正在停止…';
    case 'uncertain': return '暂时无法确认会话状态。';
    default: break;
  }
  if (info.binding?.state === 'confirmed') return '正在打开会话…';
  if (info.source !== 'shell' && sessiondockCli(info.source)?.nativeHistory !== false
      && !info.declared_sid && pendingRecordMissing(info))
    return '会话在运行，但还没找到它的记录，终端可以继续用。';
  return '';
}

// "还没找到记录"只在用户真的从本页发过消息一分钟后才说；首个回车是唯一依据。
function pendingRecordMissing(info) {
  const at = pendingFirstInput.get(info.name);
  return at !== undefined && Date.now() - at >= PENDING_RECORD_GRACE_MS;
}

function notePendingInput(name) {
  if (pendingFirstInput.has(name) || !T.pending.some(row => row.name === name)) return;
  pendingFirstInput.set(name, Date.now());
  setTimeout(() => refreshPendingStage(name), PENDING_RECORD_GRACE_MS + 50);
}

function pendingSessionRow(name) {
  return (typeof pendingTmuxSessions === 'function' ? pendingTmuxSessions() : [])
    .find(row => row.name === name)
    || T.pending.find(row => row.name === name)
    || null;
}

/** SSH 会话和代理会话同一套结构：运行中是"停止"（先 Ctrl-D，再宿主停止；行保留、
 *  录制可回放），结束后是"删除"（discard，录制一并删）。其它待定行沿用"删除"（先停再丢弃）。 */
function pendingShellRunning(row) {
  return row?.source === 'shell' && pendingPhase(row) === 'running';
}

function pendingTitle(info) {
  return info?.title || `新建 ${SOURCES[info?.source]?.name || ''} 会话`;
}

function refreshPendingStage(name) {
  if (S.sel !== pendingUid(name)) return;
  const current = pendingSessionRow(name);
  if (!current) {
    renderPendingSessionAction({ name, source: 'shell' });
    return;
  }
  const wait = $('.new-session-wait');
  if (wait) wait.textContent = pendingStageMessage(current);
  renderPendingSessionAction(current);
  if (bridge.composer.uid === pendingUid(name) && current.source !== 'shell'
      && ['exited', 'failed'].includes(current.state)
      && !$('#compose-items [data-conversation-restart]')) renderComposerItems();
}

/** 本页观察到宿主退出后，页面立刻进入结束态（头部"删除"、副标题"已结束"），
 *  并强制刷一次列表让服务端落账跟上；不等下一轮轮询。 */
function notePendingEnded(name) {
  const row = T.pending.find(row => row.name === name);
  if (row) { row.running = false; row.stale = true; }
  refreshPendingStage(name);
  if (row && typeof renderSide === 'function') renderSide();
  void loadTermList();
}

/** 选中的启动型行在两次列表之间从服务端消失（另一个页面删了它）：详情页不能
 *  留着旧头部。 */
function pendingSelectionGone(name) {
  const info = T.pending.find(row => row.name === name) || { name };
  discardAbandonedNewSession(info);
  const detail = $('#detail');
  if (detail && !S.sel) detail.innerHTML = '<div class="empty">该会话已被删除。</div>';
}

async function stopPendingSession(info, button) {
  if (SessionDockCapabilities.config.backend === 'rust') {
    if (!await appConfirm(`停止会话「${pendingTitle(info)}」?\n\n停止后才可以删除会话记录。`)) return;
    if (button) button.disabled = true;
    try {
      const result = await post('api/term/kill', {record_id: info.record_id, instance_id: info.instance_id,
        ...(HUB_MODE ? {_node: info.node_id} : {})});
      if (result.error) throw new Error(result.error);
      const current = T.pending.find(row => row.record_id === info.record_id);
      if (current) Object.assign(current, result);
      if (S.sel === pendingUid(info.name)) {
        const wait = $('.new-session-wait');
        if (wait) wait.textContent = '正在停止…';
      }
      await loadTermList();
    } catch (error) { await appAlert(error.message || '停止失败，请重试。'); }
    finally { if (button) button.disabled = false; }
    return;
  }
  return deleteSessions([pendingUid(info.name)], button);
}

async function deletePendingSession(info, button) {
  if (info.source === 'shell' && SessionDockCapabilities.config.backend === 'rust'
      && !await appConfirm(`删除会话「${pendingTitle(info)}」?\n\n会话记录和它的录制会一并删除，无法恢复。`)) return;
  if (button) button.disabled = true;
  try {
    await discardPendingSession(info);
    await loadTermList();
    if (typeof loadSessions === 'function') await loadSessions(true);
  } catch (error) { await appAlert(error.message || '删除失败，请重试。'); }
  finally { if (button) button.disabled = false; }
}

async function discardPendingSession(info) {
  if (SessionDockCapabilities.config.backend === 'rust') {
    // A finished receipt (exited/failed/cancelled) is dropped from the pending
    // view through `term/discard`; a running one must be stopped first.
    const current = T.pending.find(row => row.record_id === info.record_id) || info;
    if (current.running && !current.stale) {
      const result = await post('api/term/kill', {record_id: current.record_id, instance_id: current.instance_id,
        ...(HUB_MODE ? {_node: current.node_id} : {})});
      if (result.error) throw new Error(result.error);
      Object.assign(current, result);
    }
    const dropped = await post('api/term/discard', {record_id: current.record_id, instance_id: current.instance_id,
      ...(HUB_MODE ? {_node: current.node_id} : {})});
    if (dropped.error) throw new Error(dropped.error);
    discardAbandonedNewSession(info);
    return;
  }
  T.discarding.add(info.name);
  T.resolveControllers.get(info.name)?.abort();
  try {
    const d = await post('api/term/kill', { name: info.name });
    if (d.error || !d.ok) throw new Error(d.error || '丢弃失败');
    discardAbandonedNewSession(info);
  } finally {
    T.discarding.delete(info.name);
  }
}

/** SSH/shell receipts have no conversation archive; the PTY is the session. */
function sessionIsPtyOnly(uid = S.sel) {
  if (!uid) return false;
  const row = [...(T.pending || []), ...(T.list || [])]
    .find(item => item.uid === uid || pendingUid(item.name) === uid);
  if (row) return row.source === 'shell';
  return typeof sessionTermMeta === 'function' && sessionTermMeta(uid)?.source === 'shell';
}

/** A CLI whose conversation SessionDock cannot read yet (OpenCode) shows its
 *  replies only in the PTY: the console leads the page like SSH, while the
 *  composer keeps the CLI's own send path and input checks. */
function sessionTerminalFirst(uid = S.sel) {
  if (sessionIsPtyOnly(uid)) return true;
  if (!uid) return false;
  const row = [...(T.pending || []), ...(T.list || [])]
    .find(item => item.uid === uid || pendingUid(item.name) === uid);
  const source = row?.source || (typeof sessionTermMeta === 'function' && sessionTermMeta(uid)?.source)
    || String(uid).split(':')[0];
  return sessiondockCli(source)?.nativeHistory === false;
}

async function openPendingSession(info) {
  const pending = { ...info, name: info.tmuxName || info.name };
  showNewSessionStage(pending);
  // Agent 会话留在对话页，直到用户打开控制台。SSH 运行中默认 PTY 在
  // 输入框上方；结束后有录制则全幅只读回放。记住的布局优先。
  const running = SessionDockCapabilities.config.backend !== 'rust'
    || (pending.running && !pending.stale);
  const retained = T.views?.get(pending.name)?.keepOutput || T.views?.get(pending.name)?.ended;
  const remembered = T.openViews.has(pending.name) && running;
  const replay = pending.source === 'shell' && pending.recording && !running;
  // 已结束但有录制的 SSH 会话：控制台面板里只读回放它的录制。
  if ((pending.source === 'shell' || sessiondockCli(pending.source)?.nativeHistory === false)
      && (running || retained || pending.recording))
    await openTermPane(pending.name, true, remembered ? null : (replay ? 'full' : 'collapsed'));
  else if (remembered)
    await openTermPane(pending.name);
  resolveNewSession(pending);
}

function discardAbandonedNewSession(info) {
  const uid = pendingUid(info.name);
  T.resolveControllers.get(info.name)?.abort();
  pendingFirstInput.delete(info.name);
  T.pending = (T.pending || []).filter(x => x.name !== info.name);
  T.list = (T.list || []).filter(x => x.name !== info.name);
  const sid = info.declared_sid;
  if (sid && Array.isArray(S.sessions)) {
    S.sessions = S.sessions.filter(session => String(session.sid) !== String(sid));
  }
  T.openViews.delete(info.name);
  store.set('termviews', [...T.openViews]);
  disposeTermView(info.name);

  const draft = bridge.composer.drafts.get(uid);
  for (const attachment of draft?.attachments || []) {
    if (attachment.preview) URL.revokeObjectURL(attachment.preview);
  }
  bridge.composer.drafts.delete(uid);
  deleteComposerDraftStorage(uid);
  if (bridge.composer.uid === uid) bridge.composer.uid = null;
  syncComposerUnloadProtection();

  if (S.sel === uid) {
    const previousMode = T.pendingModes.get(info.name);
    if (['normal', 'collapsed', 'full'].includes(previousMode)) T.mode = previousMode;
    S.sel = null;
    T.uid = null;
    store.set('sel', null);
    $('#composer').classList.add('hidden');
    $('#detail').innerHTML = '<div class="empty">从左侧选择一个会话</div>';
    ensureConsolePlaceholder();
    if (typeof auditDetailRendered === 'function') auditDetailRendered('discarded', {name: info.name});
    showMobileList();
  }
  T.pendingModes.delete(info.name);
  renderSide();
  showSessionCount(sidebarSessions().length);
  paintLive();
}

async function resolveNewSession(info) {
  if (SessionDockCapabilities.config.backend === 'rust') {
    // Periodic term/list is authoritative for this launch-only view. Never run
    // filename-based resolution or automatic draft/receipt cleanup.
    const current = T.pending.find(row => row.record_id === info.record_id);
    const pendingId = pendingUid(info.name);
    // A launch that declared its full SID on the command line is associated
    // by the runtime catalog (same host name + instance nonce, immutable
    // metadata), never by cwd/time/filename; follow it to the real session
    // while keeping this page's existing pending terminal view.
    // A pending Codex/Grok launch whose binding the server confirmed
    // (process evidence, or `POST /api/term/bind`) is followed the same way.
    const associated = current && (current.declared_sid || current.binding?.state === 'confirmed');
    const terminalLinked = associated && (T.list || []).find(row => row.name === current.name
      && row.instance_id === current.instance_id && row.uid);
    // ptyhost removes its routing record immediately after exit. A durable
    // binding still names the exact native row, so follow that history even
    // when there is no terminal left to reopen.
    const nativeLinked = current?.binding?.state === 'confirmed'
      && S.sessions.find(row => row.uid === current.binding.uid
        && row.source === current.binding.source
        && String(row.sid) === String(current.binding.sid));
    const linked = terminalLinked || nativeLinked;
    if (linked && S.sel === pendingId && !S.agent) {
      migrateComposerDraft(pendingId, linked.uid);
      // The pending view holds a launch-kind lease and socket; the native
      // console claims a native lease on the same host, so release ours first.
      // Reopen it on the native session when it was open (or remembered open
      // — a mobile layout change parks the pane without forgetting it).
      const reopen = (!!ui.visible && T.name === current.name)
        || T.openViews.has(current.name);
      const existing = T.views.get(current.name);
      if (existing && existing.bindingUid === pendingId) {
        rememberTermOpen(current.name, false);
        disposeTermView(current.name);
      }
      T.uid = linked.uid;
      await openSession(linked.uid, null, {follow: true, historyMode: 'replace'});
      if (S.sel === linked.uid && !S.agent && reopen && terminalLinked) await openTermPane(current.name);
      T.pendingModes.delete(info.name);
      paintLive();
      return;
    }
    if (current && S.sel === pendingId && typeof refreshPendingStage === 'function')
      refreshPendingStage(info.name);
    return;
  }
  const pendingId = pendingUid(info.name);
  if (T.resolving.has(info.name) || T.discarding.has(info.name)) return;
  T.resolving.add(info.name);
  const controller = new AbortController();
  T.resolveControllers.set(info.name, controller);
  try {
    for (let i = 0; i < 160; i++) {         // TUI 等用户首次输入时可能较久，最多等两分钟
      await new Promise(r => setTimeout(r, 750));
      let d;
      try {
        const response = await fetch(appUrl(`api/term/new-status?name=${encodeURIComponent(info.name)}`),
          { signal: controller.signal });
        d = await response.json();
        if (controller.signal.aborted) return;
        if (HUB_MODE && response.status >= 500) continue;
      } catch {
        if (controller.signal.aborted) return;
        continue;
      }
      // 另一浏览器可能已经先清理了同一临时记录；gone 与本页观察到
      // exited 的收尾动作完全相同，不能退化成一个关联失败的孤儿页。
      if (d.exited || d.gone) {
        if (conversationSendEnabled()) {composerDraft(pendingId);await hydrateComposerDraft(pendingId);}
        const draft = bridge.composer.drafts.get(pendingId);
        if (draft && (draft.text || draft.attachments.length || draft.quotes.length)) {
          const wait = $('.new-session-wait');
          if (wait && S.sel === pendingId) wait.textContent = 'CLI 已退出，输入已保留';
        } else discardAbandonedNewSession(info);
        return;
      }
      if (d.error) {
        const wait = $('.new-session-wait');
        if (wait && S.sel === pendingId) wait.textContent = `会话关联失败：${d.error}`;
        return;
      }
      if (d.waiting) {
        const wait = $('.new-session-wait');
        if (wait && S.sel === pendingId && !d.running) wait.textContent = 'CLI 已退出，尚未生成会话记录';
        continue;
      }
      await loadSessions(true);
      await loadTermList();
      if (controller.signal.aborted) return;
      if (S.sel !== pendingId) return;      // 等待刷新期间也可能切走，不能抢走右侧页面
      migrateComposerDraft(pendingId, d.uid);
      T.uid = d.uid;
      if (d.running) {
        S.live.add(d.uid);
        S.liveTmux.add(d.uid);
      }
      await openSession(d.uid);
      if (S.sel !== d.uid || S.agent) return;
      if (d.running && T.openViews.has(d.name)) await openTermPane(d.name);
      else closeTermPane();
      T.pendingModes.delete(info.name);
      paintLive();
      return;
    }
    const wait = $('.new-session-wait');
    if (wait && S.sel === pendingId) wait.textContent = '会话仍在终端中运行；产生首条记录后会出现在列表里';
  } finally {
    T.resolving.delete(info.name);
    if (T.resolveControllers.get(info.name) === controller) T.resolveControllers.delete(info.name);
  }
}


const termRows = () => Math.max(10, Math.floor(T.height / (termFontSize() * 1.31)));

/** 详情页头部那个按钮的文案随状态变。 */
/** 终端面板每次开合/换模式都记一笔 terminal.pane：谁触发的、之前之后什么状态。 */
function auditTermPane(action, extra = {}) {
  if (typeof browserAuditEvent !== 'function') return;
  const pane = $('#termpane');
  browserAuditEvent('terminal.pane', {
    action, name: T.name, uid: T.uid, mode: T.mode, mobile: MOBILE.matches,
    visible: !!pane && !!ui.visible,
    views: [...T.views.keys()], open_views: [...T.openViews.keys()], ...extra,
  }, null, {uid: T.uid || S.sel || ''});
}

// ---------------------------------------------------------------- 终端面板
function currentTermViewObject() {
  return T.name ? T.views.get(T.name) || null : null;
}

function syncTermAliases(view = null) {
  T.term = view?.term || null;
  T.ws = view?.ws || null;
}

let termOpenEpoch = 0;

function activateTermView(view) {
  for (const cached of T.views.values()) cached.host.hidden = cached !== view;
  view.host.hidden = false;
  T.name = view.name;
  syncTermAliases(view);
  setScrollPos(view.scrollPos);
  renderTimeline(view);
  renderTermOutputNotice(view);
}

/** 只有显式打开终端的动作才能请求焦点；异步连接期间若用户已经点到别处，
 *  请求自动作废。这样 tmux 列表/会话正文的后台刷新不会打断搜索或编辑。 */
function requestTermFocus(view, source = document.activeElement) {
  view.focusRequest = source || document.body;
}

function focusTermIfRequested(view) {
  const source = view?.focusRequest;
  if (!source) return false;
  view.focusRequest = null;
  const active = document.activeElement;
  const inside = active && view.host.contains(active);
  const neutral = !active || active === document.body || active === document.documentElement;
  const editing = !inside && active?.matches?.(
    'input, textarea, select, [contenteditable="true"], [contenteditable="plaintext-only"]');
  if (editing || (!inside && !neutral && active !== source)) return false;
  view.term.focus();
  return true;
}

function legacyCopyText(text, term) {
  const input = document.createElement('textarea');
  input.value = text;
  input.setAttribute('readonly', '');
  Object.assign(input.style, {
    position: 'fixed', left: '-10000px', top: '0', opacity: '0',
  });
  document.body.appendChild(input);
  input.select();
  let copied = false;
  try { copied = document.execCommand('copy'); } finally {
    input.remove();
    term.focus();
  }
  return copied;
}

function copyTermSelection(term) {
  const text = term.getSelection();
  if (!text) return false;
  try {
    const copying = navigator.clipboard?.writeText(text);
    if (copying) copying.catch(() => legacyCopyText(text, term));
    else legacyCopyText(text, term);
  } catch {
    legacyCopyText(text, term);
  }
  return true;
}

function decodeOsc52Clipboard(payload) {
  const separator = payload.indexOf(';');
  if (separator < 0) return null;
  const selection = payload.slice(0, separator);
  const encoded = payload.slice(separator + 1);
  // Reading the workstation clipboard back into a remote process would leak
  // local data. OSC 52 set/clear is supported; the `?` query is intentionally
  // consumed without a reply.
  if (encoded === '?') return {selection, query: true};
  if (!/^[A-Za-z0-9+/]*={0,2}$/.test(encoded) || encoded.length % 4 === 1) return null;
  try {
    const binary = atob(encoded);
    const bytes = Uint8Array.from(binary, char => char.charCodeAt(0));
    return {selection, text: new TextDecoder().decode(bytes), bytes: bytes.length};
  } catch {
    return null;
  }
}

function handleOsc52Clipboard(view, payload) {
  const decoded = decodeOsc52Clipboard(payload);
  const audit = (status, method = '') => browserAuditEvent('terminal.clipboard', {
    name: view.name, operation: decoded?.query ? 'query' : 'write', status, method,
    selection: String(decoded?.selection || '').slice(0, 16), bytes: decoded?.bytes || 0,
  }, null, {uid: T.uid || '', connectionId: view.auditConnectionId || '',
    severity: status === 'failed' || status === 'malformed' ? 'warning' : 'info'});
  if (!decoded) {
    audit('malformed');
    return true;
  }
  if (decoded.query) {
    audit('ignored');
    return true;
  }
  const fallback = () => {
    if (legacyCopyText(decoded.text, view.term)) audit('copied', 'execCommand');
    else audit('failed');
  };
  try {
    const writing = navigator.clipboard?.writeText(decoded.text);
    if (writing) Promise.resolve(writing).then(
      () => audit('copied', 'clipboard'), fallback,
    );
    else fallback();
  } catch {
    fallback();
  }
  return true;
}

function rememberTermSelection(view) {
  const position = view.term.getSelectionPosition();
  if (!position || !view.term.getSelection()) return;
  view.selectionSnapshot = {
    start: { ...position.start }, end: { ...position.end },
  };
}

function restoreTermSelection(view) {
  if (!view.selectionLocked || !view.selectionSnapshot || view.term.hasSelection()
      || view.restoringSelection) return;
  view.restoringSelection = true;
  queueMicrotask(() => {
    try {
      if (!view.selectionLocked || view.term.hasSelection()) return;
      const { start, end } = view.selectionSnapshot;
      const length = (end.y - start.y) * view.term.cols - start.x + end.x;
      if (length > 0) view.term.select(start.x, start.y, length);
    } catch {
      // scrollback 已被裁掉时旧坐标可能失效，此时正常放弃锁定。
      view.selectionLocked = false;
      view.selectionSnapshot = null;
    } finally {
      view.restoringSelection = false;
    }
  });
}

function termSelectionMouseDown(event) {
  return new MouseEvent('mousedown', {
    bubbles: true, cancelable: true, composed: true, view: window,
    detail: event.detail,
    screenX: event.screenX, screenY: event.screenY,
    clientX: event.clientX, clientY: event.clientY,
    ctrlKey: event.ctrlKey, altKey: event.altKey, metaKey: event.metaKey,
    shiftKey: false, button: event.button, buttons: event.buttons,
  });
}

function shouldUseTermWebgl(uid = T.uid) {
  return !String(uid || '').startsWith('tmux:')
    && Math.abs((parseFloat(getComputedStyle(document.documentElement).zoom) || 1) - 1) < .001;
}

/** 用户选择的控制台渲染器：`grid` = 服务端网格（宿主解析，浏览器只画格子）。
 *  只有宿主声明支持网格（term/list 行的 `grid:true`）才用；旧宿主进程自动回退 xterm.js。 */
/** 这一行所在机器的控制台渲染：hub 按机器（中央注册表的 renderer），单机按本浏览器。默认服务端网格。 */
function consoleRendererFor(row) {
  if (HUB_MODE) {
    const nid = row?.node_id || (typeof newNodeId === 'function' ? newNodeId() : '');
    const node = [...(Nodes.machines || []), ...(Nodes.list || [])].find(n => n.id === nid);
    return node?.renderer === 'xterm' ? 'xterm' : 'grid';
  }
  return store.get('consoleRenderer', 'grid') === 'xterm' ? 'xterm' : 'grid';
}

function consoleRendererIsGrid(name) {
  const row = (T.list || []).find(x => x.name === name) || (T.pending || []).find(x => x.name === name);
  if (consoleRendererFor(row) !== 'grid' || typeof GridTerm !== 'function') return false;
  // 列表里还没有这一行（刚创建的会话）：新宿主一定支持网格，按偏好来。
  // 已结束但有录制的会话：回放由服务端模型驱动，两种渲染都行，也按偏好来。
  // `grid` 未知（create 回执刚本地塞进列表、还没经 term/list 补全）同样按偏好；旧宿主服务端总是显式给 false。
  if (!row || row.grid == null || (row.running === false && row.recording?.id)) return true;
  return row.grid === true;
}

function ensureTerm(name) {
  let view = T.views.get(name);
  if (view) return view;
  const host = el('div', 'xterm-view');
  host.hidden = true;
  $('#xterm').appendChild(host);
  const grid = consoleRendererIsGrid(name);
  const term = grid ? new GridTerm({
    fontFamily: termFont(), fontSize: termFontSize(), theme: termTheme(),
    cursorBlink: true, scrollback: 100000,
    onDiagnostic: (event, data) => {
      if (!view?.auditConnectionId) return;
      browserAuditEvent?.(`terminal.${event}`, {
        name, ...data, visible: termPaneRenderable(view),
        visibility: document.visibilityState,
        elapsed_ms: Math.round(performance.now() - view.auditConnectStarted),
      }, null, {uid: view.bindingUid || T.uid || '', connectionId: view.auditConnectionId});
    },
  }) : new bridge.vendors.Terminal({
    allowProposedApi: true,
    fontFamily: termFont(),
    fontSize: termFontSize(), fontWeight: '400', fontWeightBold: '600',
    rescaleOverlappingGlyphs: true,
    cursorBlink: true, scrollback: 10000,
    scrollOnUserInput: true, theme: termTheme(),
  });
  // 网格外观层没有 FitAddon：按 #xterm 容器尺寸提议行列，其余流程不变。
  const fit = grid
    ? {proposeDimensions: () => {
      const box = $('#xterm'), css = getComputedStyle(box);
      return term.proposeDimensions(box.clientWidth - parseFloat(css.paddingLeft) - parseFloat(css.paddingRight),
        box.clientHeight - parseFloat(css.paddingTop) - parseFloat(css.paddingBottom));
    }}
    : new bridge.vendors.FitAddon.FitAddon();
  view = {
    name, host, term, fit, grid, ws: null, connectTimer: null, reconnectTimer: null,
    reconnectDelay: 500, scrollPos: 0, ansiTail: '',
    fitFrame: null,
    lastResizeKey: '', lastResizeWs: null,
    activationEpoch: 0,
    attachPromise: null, revoked: false,
    focusRequest: null, resumeFocus: false,
    renderer: 'dom', webgl: null, unicode11: null,
    syncHold: null, syncHoldTimer: null,
    selectionLocked: false, selectionSnapshot: null, restoringSelection: false,
    codexSideThread: false, sideThreadScanQueued: false,
  };
  T.views.set(name, view);
  if (!grid) term.loadAddon(fit);
  if (!grid && bridge.vendors.Unicode11Addon?.Unicode11Addon) {
    try {
      view.unicode11 = new bridge.vendors.Unicode11Addon.Unicode11Addon();
      term.loadAddon(view.unicode11);
      term.unicode.activeVersion = '11';
    } catch { view.unicode11 = null; }
  }
  term.open(host);
  view.menu = mountTerminalMenu(view, {T, copyTermSelection, showConsoleToast, gestures: bridge.gestures});
  term.onScroll(() => positionTermViewport(view));
  // Edge 在任何聚焦的可编辑元素插入点旁挂一个 Copilot“撰写”浮动按钮（一个蓝点），
  // 它会贴着 xterm 这个隐藏的 IME textarea 跟随光标。Edge 124+ 认这个属性，
  // 同时关掉文本预测；其它浏览器忽略。
  term.textarea?.setAttribute('writingsuggestions', 'false');
  // Files pasted into the console are captured before the renderer's own
  // paste handler; text keeps the renderer's bracketed-paste path.
  host.addEventListener('paste', e => consolePasteFiles(view, name, e), true);
  // Claude Code uses OSC 52 after mouse selection. xterm parses the sequence
  // but has no browser clipboard policy of its own, so the embedding page must
  // opt in before Ctrl+V can paste the selected text back into the PTY.
  view.osc52 = term.parser.registerOscHandler(52, payload => handleOsc52Clipboard(view, payload));
  // WebGL 初始化是同步的，软件渲染环境可能卡住几十秒。新建/待绑定会话必须
  // 先取得控制权并连上宿主，因此其首个 view 保持 DOM renderer。原生会话仍
  // 使用 WebGL 缓解 Codex DEC ?2026 重画在 Chromium/Wayland 下的中间帧。
  if (grid && typeof term.onClipboard === 'function') {
    // 网格协议把 OSC 52 解码成文本送达；沿用 xterm 路径同一套剪贴板策略。
    term.onClipboard(text => {
      const bytes = new TextEncoder().encode(text);
      let binary = '';
      for (let i = 0; i < bytes.length; i += 0x8000) {
        binary += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
      }
      handleOsc52Clipboard(view, 'c;' + btoa(binary));
    });
  }
  if (!grid && shouldUseTermWebgl() && bridge.vendors.WebglAddon?.WebglAddon) {
    try {
      const webgl = new bridge.vendors.WebglAddon.WebglAddon();
      webgl.onContextLoss(() => {
        if (view.webgl !== webgl) return;
        view.webgl = null;
        view.renderer = 'dom';
        webgl.dispose();
        requestAnimationFrame(() => term.refresh(0, term.rows - 1));
      });
      term.loadAddon(webgl);
      view.webgl = webgl;
      view.renderer = 'webgl';
    } catch { /* WebGL2/硬件加速不可用时保留 DOM renderer */ }
  }
  const forwardedSelectionStarts = new WeakSet();
  host.addEventListener('mousedown', e => {
    if (forwardedSelectionStarts.has(e) || e.button !== 0) return;
    if (e.shiftKey || term.modes.mouseTrackingMode === 'none') {
      // Complete the local selection before copying, including releases
      // outside the terminal. Remote CLI mouse gestures never enter here.
      window.addEventListener('mouseup', () => {
        view.selectionLocked = false;
        view.selectionSnapshot = null;
        copyTermSelection(term);
        term.clearSelection();
      }, {once: true});
    }
    view.selectionLocked = e.shiftKey;
    if (!e.shiftKey) return;
    view.selectionSnapshot = null;
    // VT mouse 开启时 xterm 自己用 Shift 强制进入本地选择，必须保留原事件，
    // 才不会把鼠标发给 vim/less 等里面的程序。
    if (term.modes.mouseTrackingMode !== 'none') return;
    // xterm 把 Shift+拖拽解释为“扩展已有选区”，没有旧选区时结果为空。
    // tmux 用户则用 Shift 绕过终端鼠标模式并开始一次新框选。拦住原事件，
    // 以普通左键事件启动 xterm 自己的选择器；后续 move/up 仍由它原样处理。
    e.preventDefault();
    e.stopImmediatePropagation();
    const forwarded = termSelectionMouseDown(e);
    forwardedSelectionStarts.add(forwarded);
    e.target.dispatchEvent(forwarded);
  }, true);
  term.onSelectionChange(() => {
    if (term.hasSelection()) rememberTermSelection(view);
    else restoreTermSelection(view);       // Claude 重绘清选区时，恢复刚才的框选
  });
  term.attachCustomKeyEventHandler(e => {
    if (e.code === 'ControlRight') {
      if (e.type === 'keydown' && !e.repeat) setTermCtrl(true);
      return false;                        // 右 Ctrl 只锁定下一键，不交给 xterm
    }
    const paste = (e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === 'v';
    if (paste) {
      if (e.type === 'keydown') {
        view.selectionLocked = false;
        view.selectionSnapshot = null;
      }
      return false;                       // 交给浏览器派发 paste，xterm 再做 bracketed paste
    }
    if (e.type === 'keydown' && !['Control', 'Shift', 'Alt', 'Meta'].includes(e.key)) {
      view.selectionLocked = false;
      view.selectionSnapshot = null;
    }
    return true;
  });
  term.onData(d => {
    if (T.name !== name || view.replay) return;   // 录制回放只读
    d = applyTermAlt(applyTermCtrl(d));
    d = stripOscColorReports(d);
    if (!d) return;
    if (view.ws?.readyState !== 1) return;
    browserAuditEvent('terminal.input', {name, bytes: new TextEncoder().encode(d).length},
      d, {uid: T.uid || '', connectionId: view.auditConnectionId || ''});
    if (view.scrollPos || _wheelRequests.size || _resumeInput) {
      // 等所有已经发出的滚轮请求落地，再由一个服务端请求原子执行
      // 「退出 copy-mode → 写入字符」。直接向 attach 发 q 不可靠，而把
      // cancel 和字符分走 HTTP/WS 两条通道又会乱序。
      abortWheel();
      setScrollPos(0);
      const before = _resumeInput || Promise.allSettled([..._wheelRequests]);
      const body = termInputBody(name, { name, text: d, enter: false });
      const job = before.then(() => post('api/term/send', body));
      _resumeInput = job;
      job.then(
        () => { if (_resumeInput === job) _resumeInput = null; },
        () => { if (_resumeInput === job) _resumeInput = null; },
      );
      return;
    }
    view.ws.send(new TextEncoder().encode(d));
    if (/[\r\n]/.test(d)) notePendingInput(name);
  });
  // ptyhost 没有服务端 copy-mode；滚轮交给 grid/xterm 的历史或应用鼠标处理。
  // 改造前遗留在默认 tmux server 的会话仍走旧兼容路径。
  term.attachCustomWheelEventHandler(e => {
    if (T.name !== name) return true;
    // 录制回放和已退出的画面没有宿主 copy-mode；滚轮留给 xterm / 面板滚动条。
    if (view.replay || view.ended || view.revoked) return true;
    const server = T.list?.find(x => x.name === name)?.server;
    if (server === 'ptyhost' || server === 'sessiondock') return true;
    wheelBy(e.deltaY);
    return false;
  });
  return view;
}

// 每个 WebSocket 包原样立即交给 xterm，页面这层不再攒 20 ms 合帧：xterm 自己
// 的 WriteBuffer 已按帧合并解析，Claude Code / Codex 的整屏重画都包在
// DEC 2026（synchronized output）里，由 xterm 压到一帧内绘制，不会再画出
// “先清行后重写”的中间态。攒批只会让每次按键回显固定多等一个定时器
// （实测 localhost p50 从 ~30 ms 降到 <1 ms，见 tests/bench_term_echo_browser.py）。
function writeTermOutput(view, chunk) {
  if (!chunk) return;
  // 网格视图收到的是 JSON 行，没有转义序列，也不需要攒同步帧。
  if (view.grid) {
    writeParsedTermOutput(view, chunk);
    return;
  }
  // 亮色页面只反射“深底/浅字”的显式颜色；Codex 已是浅色的 diff 保持原样。
  // 暗色页面与默认/ANSI 16 色仍交给 termTheme。xterm 会跨 write 保留 SGR 状态。
  chunk = terminalColorChunk(view, chunk);
  if (!chunk) return;
  // 唯一的例外：一个 ?2026h 打开、还没 ?2026l 收尾的同步帧整帧攒住再写。xterm
  // 的绘制虽然已按 2026 合帧，但它每 parse 一个 write 就把隐藏的输入 textarea
  // 挪到当时的光标格；Claude 的一帧常拆成几个包，中间光标在清行时来回跳，
  // 浏览器贴在 textarea 上的原生小部件（触屏选择把手等）就跟着满屏乱闪。
  // 按键回显不带 2026，仍然直写。
  if (view.syncHold !== null) {
    view.syncHold += chunk;
  } else if (termSyncFrameOpen(chunk)) {
    view.syncHold = chunk;
    view.syncHoldTimer = setTimeout(() => flushTermSyncHold(view), TERM_SYNC_HOLD_MS);
  } else {
    writeParsedTermOutput(view, chunk);
    return;
  }
  if (termSyncFrameOpen(view.syncHold) && view.syncHold.length < TERM_SYNC_HOLD_MAX) return;
  flushTermSyncHold(view);
}

// 最后一个 ?2026h 之后没有 ?2026l 就算帧还开着。
function termSyncFrameOpen(s) {
  return s.lastIndexOf('\x1b[?2026h') > s.lastIndexOf('\x1b[?2026l');
}

function flushTermSyncHold(view) {
  if (view.syncHoldTimer) clearTimeout(view.syncHoldTimer);
  view.syncHoldTimer = null;
  const held = view.syncHold;
  view.syncHold = null;
  if (held) writeParsedTermOutput(view, held);
}

/** Codex 的 side thread 目前可能只存在于正在运行的 TUI，绑定 main thread 的
 * 原生记录不会随它增长。只检查 xterm 已解析的实时屏幕底部，不能从原始包或
 * scrollback 搜索：重绘包会带旧内容，用户向上滚动也不代表 CLI 已切线程。 */
function terminalViewportHasCodexSideThread(term) {
  const buffer = term?.buffer?.active;
  const rows = Math.max(0, Number(term?.rows) || 0);
  if (!buffer || !rows) return false;
  const end = Math.min(Number(buffer.length) || 0, (Number(buffer.baseY) || 0) + rows);
  const start = Math.max(0, end - Math.min(rows, 12));
  let text = '';
  for (let row = start; row < end; row++) {
    text += `${buffer.getLine(row)?.translateToString(true) || ''}\n`;
  }
  return text.includes('Side from main thread');
}

function codexSideThreadVisible(uid = S.sel) {
  return [...T.views.values()].some(view => termBindingServes(view.bindingUid, uid)
    && view.codexSideThread && !view.ended && !view.retired);
}

function setCodexSideThreadState(view, active) {
  active = !!active;
  if (view.codexSideThread === active) return;
  view.codexSideThread = active;
  const uid = S.sel || '';
  if (uid && termBindingServes(view.bindingUid, uid) && !S.agent
      && typeof renderConversationTail === 'function') {
    renderConversationTail(cache.get(viewKey(uid))?.activity || null, uid);
  }
}

function scheduleCodexSideThreadScan(view) {
  if (view.sideThreadScanQueued) return;
  view.sideThreadScanQueued = true;
  queueMicrotask(() => {
    view.sideThreadScanQueued = false;
    if (!T.views.has(view.name)) return;
    setCodexSideThreadState(view, terminalViewportHasCodexSideThread(view.term));
  });
}

function writeParsedTermOutput(view, chunk) {
  view.term.write(chunk, () => {
    if (view.outputLayoutFrame) return;
    view.outputLayoutFrame = requestAnimationFrame(() => {
      view.outputLayoutFrame = null;
      if (T.views.get(view.name) !== view) return;
      scheduleCodexSideThreadScan(view);
      positionTermViewport(view);
    });
  });
}

// Keep the PTY size stable under a soft keyboard, but do not bottom-align its
// blank tail. Short menus fit from the top; tall screens follow their content
// and cursor. Use parsed cells for both renderers, not CLI-specific text rules.
function positionTermViewport(view) {
  if (!termPaneRenderable(view)) return;
  const host = view.host, term = view.term, buffer = term.buffer?.active;
  let offset = 0;
  if (!view.replay && visualKeyboardOpen() && buffer) {
    const screen = host.querySelector(view.grid ? 'canvas' : '.xterm-screen');
    const zoom = parseFloat(getComputedStyle(document.documentElement).zoom) || 1;
    const height = (screen?.getBoundingClientRect().height || 0) / zoom;
    const cellHeight = height / term.rows;
    const top = buffer.viewportY;
    const cursor = buffer.baseY + buffer.cursorY - top;
    let last = -1;
    for (let row = term.rows - 1; row >= 0; row--) {
      if (buffer.getLine(top + row)?.translateToString(true).trim()) { last = row; break; }
    }
    const cursorInView = cursor >= 0 && cursor < term.rows;
    if (cursorInView) last = Math.max(last, cursor);
    offset = Math.max(0, (last + 1) * cellHeight - host.clientHeight);
    // A status footer must not push an editor/menu cursor above the pane.
    if (cursorInView) offset = Math.min(offset, cursor * cellHeight);
    offset = Math.min(offset, Math.max(0, height - host.clientHeight));
  }
  host.style.setProperty('--terminal-viewport-offset', `${-offset}px`);
}

function dropTermSyncHold(view) {
  if (view.syncHoldTimer) clearTimeout(view.syncHoldTimer);
  view.syncHoldTimer = null;
  view.syncHold = null;
}

function termPaneRenderable(view = currentTermViewObject()) {
  if (!view || view !== currentTermViewObject()) return false;
  const pane = $('#termpane');
  if (!ui.visible) return false;
  if (!MOBILE.matches && T.mode === 'collapsed' && !(typeof sessionTerminalFirst === 'function' && sessionTerminalFirst()))
    return false;
  if (ui.collapsed) return false;
  // 手机从桌面布局切回会话列表时，#right 会由祖先的 display:none 隐藏，
  // 但 #termpane 本身没有 hidden 类。bridge.vendors.FitAddon 在这种容器上会返回内部最小值
  // 10×5；先确认当前 host 真正参与布局，不能让这组伪尺寸污染 xterm/PTY。
  const host = view.host;
  if (!host || host.hidden || !host.isConnected) return false;
  const rect = host.getBoundingClientRect();
  return rect.width > 0 && rect.height > 0;
}

function repaintTermView(view) {
  try { view.term.refresh(0, Math.max(0, view.term.rows - 1)); } catch { /* disposed */ }
}

function performTermFit(view, forceSync = false) {
  if (!termPaneRenderable(view)) return;
  positionTermViewport(view);
  // 录制回放按录制时的尺寸呈现，不随面板大小重排（服务端也不接受 resize）。
  if (view.replay) {
    const size = view.replaySize;
    if (size && (view.term.cols !== size.cols || view.term.rows !== size.rows)) view.term.resize(size.cols, size.rows);
    // Older grid recordings included container padding in their column count.
    // Fit their pixels to the available width without changing recorded cells.
    if (view.grid && size) {
      const box = $('#xterm'), css = getComputedStyle(box);
      const width = box.clientWidth - parseFloat(css.paddingLeft) - parseFloat(css.paddingRight);
      view.term.options.fontSize = termFontSize();
      const recordedWidth = view.term.renderer.cellWidth * size.cols;
      if (recordedWidth > width && width > 0) {
        view.term.options.fontSize = termFontSize() * width / recordedWidth;
        // Grid cells round to device pixels, which can round the width up.
        while (view.term.renderer.cellWidth * size.cols > width && view.term.options.fontSize > 1)
          view.term.options.fontSize = Math.max(1, view.term.options.fontSize - .25);
      }
      view.term.renderer.fit(size.cols * view.term.renderer.cellWidth,
        size.rows * view.term.renderer.cellHeight);
      repaintTermView(view);
    }
    if (forceSync) repaintTermView(view);
    return;
  }
  // 软键盘会把 bridge.vendors.FitAddon 量到的行数砍掉一截。把这个尺寸发给 PTY 会让 CLI
  // 重排并丢掉编辑区，会话模式 CHECK/SEND 随即 409 cli_not_ready。网页已经
  // 按 visual viewport 让位，这里按键盘收起时的布局取行列，只把画面钉在提示符；
  // 键盘开着时的界面缩放仍是真实布局变化，行列照常跟随。
  const keyboard = typeof visualKeyboardOpen === 'function' && visualKeyboardOpen();
  let dimensions;
  try {
    dimensions = keyboard ? measureKeyboardClosedLayout(() => view.fit.proposeDimensions())
      : view.fit.proposeDimensions();
  } catch { return; }
  if (keyboard) {
    try { view.term.scrollToBottom(); } catch { /* disposed */ }
  }
  if (!dimensions || !Number.isFinite(dimensions.cols) || !Number.isFinite(dimensions.rows)) {
    if (keyboard) positionTermViewport(view);
    return;
  }
  // bridge.vendors.FitAddon.fit() 会先调用私有 _renderService.clear()，DOM renderer 因而在每次
  // 窗口缩放时先变空再重画。直接使用公开 resize API 保留旧行，并平滑增删行列。
  const resized = view.term.cols !== dimensions.cols || view.term.rows !== dimensions.rows;
  if (resized) {
    view.term.resize(dimensions.cols, dimensions.rows);
  }
  const ws = view.ws;
  const key = `${view.term.cols}x${view.term.rows}`;
  // 同一个 socket 的相同尺寸不再反复通知 tmux，避免 TUI 收到无效 SIGWINCH。
  if (ws?.readyState === 1 && (forceSync
      || view.lastResizeWs !== ws || view.lastResizeKey !== key)) {
    ws.send(JSON.stringify({ t: 'resize', cols: view.term.cols, rows: view.term.rows }));
    view.lastResizeWs = ws;
    view.lastResizeKey = key;
  }
  // display:none 下缓存的 WebGL/DOM surface 可能失去内容；若行列数碰巧没变，
  // bridge.vendors.Terminal.resize 不会触发 renderer。重新激活时必须显式画回整个 viewport。
  if (resized || forceSync) repaintTermView(view);
  if (keyboard) positionTermViewport(view);
}

function refreshTerminalScale(settled = false) {
  // xterm's WebGL atlas uses window DPR alone. DOM text is rasterized by the
  // browser at CSS zoom, so scaled views must not stretch the old glyph atlas.
  const zoom = parseFloat(getComputedStyle(document.documentElement).zoom) || 1;
  for (const view of T.views.values()) {
    // Removing a canvas mid-pinch cancels touches anchored to it. Switch only
    // after the gesture ends; grid canvases can be repainted in place throughout.
    if (settled && view.webgl && Math.abs(zoom - 1) >= .001) {
      const webgl = view.webgl;
      view.webgl = null;
      view.renderer = 'dom';
      webgl.dispose();
    }
  }
  fitTerm();
  const view = currentTermViewObject();
  if (termPaneRenderable(view)) repaintTermView(view);
}

function fitTerm(immediate = false, forceSync = false) {
  const view = currentTermViewObject();
  if (!termPaneRenderable(view)) return;
  if (view.fitFrame) clearTimeout(view.fitFrame);
  view.fitFrame = null;
  if (immediate) {
    performTermFit(view, forceSync);
    return;
  }
  // Wait for a brief pause in boundary dragging before reflowing scrollback.
  // 软键盘只改网页可视高度，performTermFit 会拒绝随之 SIGWINCH。
  view.fitFrame = setTimeout(() => {
    view.fitFrame = null;
    performTermFit(view, forceSync);
  }, 100);
}

function settleActivatedTermView(view) {
  const epoch = ++view.activationEpoch;
  const verify = () => {
    if (view.activationEpoch !== epoch || !termPaneRenderable(view)) return;
    performTermFit(view);
    repaintTermView(view);
  };
  // 先等浏览器提交“hidden → visible”和详情页布局，再复核一次；字体或滚动条
  // 稍晚稳定的浏览器再由短定时器兜底。两次都受 activationEpoch 约束。
  requestAnimationFrame(() => requestAnimationFrame(verify));
  setTimeout(verify, 120);
}

function currentTermView(name = T.name) {
  const row = SessionDockCapabilities.config.backend === 'rust'
    ? (String(T.uid || '').startsWith('tmux:') ? (T.pending || []) : T.list).find(row => row.name === name) : null;
  return { mode: T.mode, height: T.height,
    ...(row ? {uid: row.uid || (row.record_id && pendingUid(row.name)), instance_id: row.instance_id,
      ...(row.record_id ? {record_id: row.record_id, launch_id: row.launch_id} : {})} : {}) };
}

function rememberTermLayout(name = T.name) {
  if (!name || !T.openViews.has(name)) return;
  T.openViews.set(name, currentTermView(name));
  store.set('termviews', [...T.openViews]);
}

function rememberTermOpen(name, open) {
  if (!name) return;
  const changed = open ? !T.openViews.has(name) : T.openViews.has(name);
  if (!changed) return;
  if (open) T.openViews.set(name, currentTermView(name));
  else T.openViews.delete(name);
  store.set('termviews', [...T.openViews]);
}

function restoreTermPane(uid, agent = null) {
  if (SessionDockCapabilities.config.backend === 'rust' && T.ended.has(uid)) return;
  if (!uid || agent || S.sel !== uid || !sessionTerminalEnabled(uid) || !$('#a-term')) return;
  const name = takenOver(uid);
  if (!name || !T.openViews.has(name)) return;
  T.uid = uid;
  const prompt = cache.get(viewKey(uid))?.prompt || null;
  // 刷新页面时可能先恢复了一个仍在等待回答的原生题目，再恢复终端布局。
  // 先登记并呈现题目，不能让旧的纯终端偏好随后把题卡重新盖住。
  const promptId = String(prompt?.id || '');
  const waitingPrompt = !!promptId && prompt?.questions?.length
    && (prompt.state || 'waiting') === 'waiting';
  if (waitingPrompt && !ui.visible) {
    revealedTermPrompts.set(uid, promptId);
    if (MOBILE.matches) {
      renderTakeoverBtn();
      return;
    }
    const savedMode = T.openViews.get(name)?.mode;
    if (savedMode !== 'normal') {
      openTermPane(name, false, 'collapsed', true);
      return;
    }
  }
  if (revealConversationForPrompt(uid, prompt)) return;
  // loadTermList 和会话 reset 都会走这里。已打开时无需反复 fit/聚焦；首次
  // 自动恢复也只恢复视图，不应抢走用户正在使用的搜索框或输入框。
  if (!!ui.visible && T.name === name) {
    renderTakeoverBtn();
    return;
  }
  auditTermPane('restore', {target: name});
  openTermPane(name, false, null, true);
}

async function loadTerminalRenderer(name) {
  try {
    await bridge.vendors.ensureAssets?.(consoleRendererIsGrid(name));
    return true;
  } catch (error) {
    const uid = T.views.get(name)?.bindingUid
      || T.list?.find(row => row.name === name)?.uid || T.uid || S.sel;
    const message = '控制台组件加载失败，请再次打开终端重试：' + (error.message || error);
    ConsoleUI.errors.set(uid, message);
    renderTakeoverBtn();
    if (typeof showConsoleToast === 'function' && uid === S.sel) showConsoleToast(message);
    return false;
  }
}

async function openTermPane(name, autoFocus = true, requestedMode = null, auto = false, directClaim = false) {
  // An explicit switch to the terminal acknowledges the question already on
  // the conversation page. A later list refresh must not reveal it again and
  // undo that choice; a newly arriving question ID can still reveal itself.
  if (!auto && T.uid === S.sel && !S.agent) {
    const prompt = cache.get(viewKey(T.uid))?.prompt;
    if (prompt?.id && prompt.questions?.length && (prompt.state || 'waiting') === 'waiting') {
      revealedTermPrompts.set(T.uid, String(prompt.id));
    }
  }
  // 同一宿主上另一类控制台（如已关联前的等待页）还连着时，先放开它；
  // 原生控制台随后按自己的租约重新连接，和跨页面抢占走同一条路。
  const existing = T.views.get(name);
  if (SessionDockCapabilities.config.backend === 'rust' && existing?.bindingUid
      && !termBindingServes(existing.bindingUid, T.uid)) {
    rememberTermOpen(name, false);
    disposeTermView(name);
  }
  const openEpoch = ++termOpenEpoch;
  auditTermPane('open', {target: name, requested_mode: requestedMode, auto_focus: autoFocus, auto});
  const focusSource = autoFocus ? document.activeElement : null;
  const saved = T.openViews.get(name);
  if (['normal', 'collapsed', 'full'].includes(requestedMode)) {
    T.mode = requestedMode;
  } else if (saved) {
    if (['normal', 'collapsed', 'full'].includes(saved.mode)) T.mode = saved.mode;
    if (Number.isFinite(saved.height) && saved.height > 0) T.height = saved.height;
  } else if (!MOBILE.matches) {
    // 新接管或从未保存过布局的会话默认只显示终端；分屏只能由拖动产生。
    T.mode = 'full';
  }
  rememberTermOpen(name, true);
  rememberTermLayout(name);
  const pane = $('#termpane');
  ui.visible = true; publish();
  layoutTermPane();
  renderTakeoverBtn();
  try { await terminalFontReady; } catch { /* 字体失败时继续用 Consola/monospace */ }
  if (!await loadTerminalRenderer(name)) {
    if (openEpoch === termOpenEpoch) {
      ui.visible = false; publish();
      $('#right').classList.remove('term-full');
      renderTakeoverBtn();
    }
    return false;
  }
  if (openEpoch !== termOpenEpoch || !ui.visible) return false;
  const view = ensureTerm(name);
  if (autoFocus) requestTermFocus(view, focusSource);
  activateTermView(view);
  // 桌面 Agent「纯对话」吸附态高度为 0。此时保留 xterm 对象和已有连接，但不要
  // 新连或 fit；否则内部最小尺寸会把真实 tmux pane 压成 10×6。
  if (termPaneRenderable(view)) {
    // 缓存 view 即使行列数相同也可能丢了 renderer surface；强制同步并重绘。
    fitTerm(true, true);                 // 连接前先确定尺寸，避免 80×24 → 实际尺寸的首屏跳变
    settleActivatedTermView(view);
    if (view.ws?.readyState !== 1) await attachTerm(name, auto, directClaim);
    else focusTermIfRequested(view);
  }
  return true;
}

/** 顶栏按钮只切纯对话/纯终端；normal 分屏只能由用户拖动分界线产生。 */
function toggleTermPane(name) {
  const pane = $('#termpane');
  auditTermPane('toggle', {target: name});
  if (!ui.visible) {
    return openTermPane(name, true, MOBILE.matches ? null : 'full');
  }
  if (typeof sessionTerminalFirst === 'function' && sessionTerminalFirst()) {
    T.mode = T.mode === 'full' ? 'collapsed' : 'full';
    store.set('termmode', T.mode);
    rememberTermLayout(name);
    layoutTermPane();
    renderTakeoverBtn();
    if (T.mode === 'full') return openTermPane(name);
    fitTerm(true, true);
    return;
  }
  if (!MOBILE.matches) {
    // 分屏状态点按钮也进入纯终端；下一次再切到纯对话。
    T.mode = T.mode === 'full' ? 'collapsed' : 'full';
    store.set('termmode', T.mode);
    rememberTermLayout(name);
    layoutTermPane();
    renderTakeoverBtn();
    if (T.mode === 'full') return openTermPane(name);
    return;
  }
  closeTermPane();
}

// 同一个题目只自动呈现一次。用户看过以后仍可以主动切回原生终端；后台的
// 0.4 秒审批轮询和 tmux 列表刷新不能再把界面强行切回来。
const revealedTermPrompts = new Map();

/** 纯终端会遮住对话题卡。当前会话第一次收到待回答题目时切到对话；手动
 *  split 本来就能同时看到题卡，不改变它。prompt 结束后允许同一命令再次提问。 */
function revealConversationForPrompt(uid, prompt) {
  const id = String(prompt?.id || '');
  const waiting = !!id && prompt?.questions?.length
    && (prompt.state || 'waiting') === 'waiting';
  if (!waiting) {
    revealedTermPrompts.delete(uid);
    return false;
  }
  if (S.sel !== uid || S.agent) return false;
  if (typeof sessionIsPtyOnly === 'function' && sessionIsPtyOnly(uid)) return false;
  const pane = $('#termpane');
  if (!pane || !ui.visible) return false;
  if (revealedTermPrompts.get(uid) === id) return false;
  revealedTermPrompts.set(uid, id);
  auditTermPane('prompt-reveal', {prompt: id});
  if (MOBILE.matches) {
    closeTermPane(true);
    return true;
  }
  if (T.mode !== 'full') return false;
  T.mode = 'collapsed';
  store.set('termmode', T.mode);
  rememberTermLayout(T.name || takenOver(uid));
  layoutTermPane();
  renderTakeoverBtn();
  return true;
}

function closeTermPane(preserveView = false) {
  auditTermPane('close', {preserve_view: preserveView});
  termOpenEpoch++;                       // 令仍在等待字体/连接的旧 openTermPane 作废
  if (preserveView) rememberTermLayout();
  else rememberTermOpen(T.name, false);
  deactivateTermView();
  const pane = $('#termpane');
  ui.visible = false; publish();
  ui.collapsed = false; publish();
  $('#right').classList.remove('term-full');
  renderTakeoverBtn();
}

function layoutTermPane() {
  const pane = $('#termpane');
  const right = $('#right');
  const desktop = !MOBILE.matches;
  const paneOpen = !!ui.visible;
  const shell = typeof sessionTerminalFirst === 'function' && sessionTerminalFirst();
  right.classList.toggle('shell-session', typeof sessionIsPtyOnly === 'function' && sessionIsPtyOnly());
  right.classList.toggle('terminal-first', shell);
  right.classList.toggle('term-full', paneOpen && T.mode === 'full' && (desktop || shell));
  if (shell && paneOpen && T.mode !== 'full') {
    ui.collapsed = false; publish();
    ui.mobileTop = '';
    const detailHeight = $('#detail')?.offsetHeight || 0;
    const composerHeight = $('#composer')?.offsetHeight || 0;
    ui.height = Math.max(0, right.clientHeight - detailHeight - composerHeight) + 'px';
    publish(); return;
  }
  ui.collapsed = desktop && paneOpen && T.mode === 'collapsed';
  if (MOBILE.matches) {
    ui.height = '';
    const rightTop = right.getBoundingClientRect().top;
    const headBottom = $('#detail > .dhead')?.getBoundingClientRect().bottom ?? rightTop;
    // Round toward the header: rounding down overlaps its opaque background;
    // rounding up exposes a strip of message text below it at fractional zoom.
    ui.mobileTop = `${Math.max(0, Math.floor(headBottom - rightTop))}px`;
  } else {
    ui.mobileTop = '';
    if (T.mode === 'collapsed') ui.height = '0px';
    else if (T.mode === 'full') {
      ui.height = Math.max(0, right.clientHeight - $('#detail').offsetHeight) + 'px';
    }
    else {
      const detailHeadHeight = $('#detail > .dhead')?.offsetHeight || 0;
      const composerHeight = $('#composer')?.offsetHeight || 0;
      const maxHeight = Math.max(0, right.clientHeight - detailHeadHeight - composerHeight);
      ui.height = Math.min(T.height, maxHeight) + 'px';
    }
  }
  publish();
}

// `auto`：由布局恢复（选中会话、刷新、题卡）自动打开的 pty。进入对话页不该被
// 抢占问题打断：别处持有时静默放弃、留在对话页；只有用户主动打开 pty 才问。
async function claimTermOwnership(name, uid = T.uid, binding = {}, auto = false, direct = false) {
  const claim = async force => {
    try {
      return await post('api/term/claim', {name, page: TERM_PAGE_ID,
        ...(force ? {force: true} : {}), ...binding}, {timeoutMs: TERM_CLAIM_TIMEOUT_MS});
    } catch (error) {
      if (auto || error.name !== 'TimeoutError') throw error;
      return {timeout: true, error: '控制台控制权请求超时；服务端可能已取得控制权。请重新打开控制台核对状态。'};
    }
  };
  let result = await claim(direct);
  if (result.conflict && !direct) {
    if (auto) {
      auditTermPane('restore-held', {target: name, by: result.owner?.label || ''});
      return null;
    }
    const holder = describeTermTaker(result.owner?.label,
      result.same_address === false ? result.owner?.ip : '');
    if (!await appConfirm(`该终端正由${holder}控制。\n\n是否抢占终端？`)) return null;
    result = await claim(true);
  }
  if (result.error || !result.token) {
    ConsoleUI.errors.set(uid, result.error || '无法取得终端控制权');
    renderTakeoverBtn();
    if (!auto && !result.timeout) await appAlert('打开终端失败：' + (result.error || '无法取得终端控制权'));
    return null;
  }
  ConsoleUI.errors.delete(uid);
  return result.token;
}

// 抢占方的描述：浏览器拿不到主机名/用户名，服务端能给的只有 User-Agent 推出的
// 设备标签（"iPhone · Safari"）和地址；经 hub 访问时同一用户各页面地址相同，
// 所以服务端只在地址与本页不同时才给出地址。两样都没有就只说"另一页面"。
function describeTermTaker(label = '', ip = '') {
  const where = ip ? `（${ip}）` : '';
  return label ? ` ${label}${where} ` : `另一页面${where}`;
}

function handleTermRevoked(view, ip = '', by = '') {
  if (view.revoked) return;
  view.revoked = true;
  auditTermPane('revoked', {target: view.name, by: ip, label: by});
  cancelTermReconnect(view);
  if (T.name === view.name) closeTermPane();
  try { view.ws?.close(); } catch {}
  appAlert(`终端已被${describeTermTaker(by, ip)}抢占，本页面的终端已关闭。`);
}

function renderTermOutputNotice(view) {
  ui.outputNotice = view?.outputNotice || '';
  publish();
}

function recordHostExit(view, uid, event) {
  if (SessionDockCapabilities.config.backend !== 'rust') return false;
  const incomplete = event.code === 1011 && event.reason.startsWith('host output incomplete');
  if (!incomplete && !(event.code === 1000 && event.reason === 'host exited')) return false;
  const pendingRow = (T.pending || []).find(item => item.name === view.name);
  const shell = pendingRow?.source === 'shell'
    || (typeof sessionTerminalFirst === 'function' && sessionTerminalFirst(uid));
  const reason = incomplete
    ? `终端输出不完整：${event.reason}。已保留收到的尾部输出，不会自动重新连接。`
    : shell ? '终端进程已退出，已保留收到的输出。' : 'CLI 已退出，终端已关闭。';
  const keepPane = shell && T.name === view.name;
  view.ended = true;
  view.revoked = true; // An explicitly exited instance must never be auto-claimed.
  cancelTermReconnect(view);
  if (!keepPane) rememberTermOpen(view.name, false);
  T.ended.set(uid, {instanceId: view.instanceId, reason});
  if (T.ended.size > 256) T.ended.delete(T.ended.keys().next().value);
  ConsoleUI.errors.set(uid, reason);
  if (String(uid).startsWith('tmux:')) notePendingEnded(view.name);
  if (incomplete) {
    // GridTerm accepts grid JSON, not terminal text. Keep its host screen
    // intact and show the diagnostic beside it within the same console pane.
    if (view.grid) {
      view.outputNotice = reason;
      if (T.name === view.name) renderTermOutputNotice(view);
    } else {
      try { view.term.write(`\r\n${reason}\r\n`); } catch { /* disposed view */ }
    }
  } else {
    // AI sessions close the pane and return to the conversation. SSH/shell
    // keeps the console and, when a recording exists, switches it to
    // read-only replay with the timeline — the live tail is not the archive.
    view.keepOutput = shell;
    if (!shell) disposeTermView(view.name);
    else if (keepPane) {
      T.mode = 'full';
      if (typeof rememberTermLayout === 'function') rememberTermLayout(view.name);
      if (typeof layoutTermPane === 'function') layoutTermPane();
      if (!(typeof startShellRecordingReplay === 'function' && startShellRecordingReplay(view, uid))
          && typeof loadTermList === 'function') {
        void Promise.resolve(loadTermList()).then(() => {
          if (T.views.get(view.name) === view && !view.replay
              && typeof startShellRecordingReplay === 'function')
            startShellRecordingReplay(view, uid);
        });
      }
    }
    const stopNotice = document.querySelector('#session-stop-notice');
    if (!shell && uid === S.sel && typeof showSessionStopNotice === 'function'
        && (!stopNotice || stopNotice.hidden)) showSessionStopNotice(reason);
  }
  renderTakeoverBtn();
  return true;
}

/** 在控制台面板里只读回放一段录制：不 claim、不发输入；网格视图收 JSON 行，xterm 视图收净化后的字节。 */
function startShellRecordingReplay(view, uid) {
  if (typeof attachRecordingReplay !== 'function' || !view) return false;
  if (view.replay && view.ws && view.ws.readyState < 2) return true;
  const row = (T.pending || []).find(item => item.name === view.name);
  if (!row?.recording?.id) return false;
  if (T.name === view.name) T.mode = 'full';
  attachRecordingReplay(view, row, uid);
  if (T.name === view.name) {
    if (typeof rememberTermLayout === 'function') rememberTermLayout(view.name);
    if (typeof layoutTermPane === 'function') layoutTermPane();
    renderTimeline(view);
  }
  return true;
}

function sessionRecordingReplayable(uid) {
  if (!uid || typeof T === 'undefined') return false;
  const name = String(uid).startsWith('tmux:')
    ? String(uid).slice(5)
    : (typeof takenOver === 'function' ? takenOver(uid) : '');
  const view = name ? T.views?.get(name) : null;
  if (view?.replay) return true;
  const row = (T.pending || []).find(item =>
    item.name === name
    || (typeof pendingUid === 'function' && pendingUid(item.name) === uid));
  return !!(row?.recording?.id && typeof pendingPhase === 'function'
    && (pendingPhase(row) === 'exited' || pendingPhase(row) === 'failed'));
}

/** 录制回放的时间轴：进度条、播放/暂停、倍速、跳到最新。只对当前视图画。 */
function replayTimeElapsed(ms) {
  const total = Math.max(0, Math.round(ms / 1000));
  const h = Math.floor(total / 3600), m = Math.floor((total % 3600) / 60), sec = total % 60;
  const mm = String(m).padStart(2, '0'), ss = String(sec).padStart(2, '0');
  return h ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}

function renderTimeline(view = currentTermViewObject()) {
  const tl = view?.replay ? view.timeline : null;
  ui.replay = !!tl;
  if (!tl) { publish(); return; }
  const span = Math.max(0, tl.end - tl.start);
  if (!tl.scrubbing) ui.timeline.seek = span ? String(Math.round((tl.clock - tl.start) / span * 1000)) : '1000';
  const at = tl.scrubbing ? tl.start + Number(ui.timeline.seek) / 1000 * span : tl.clock;
  Object.assign(ui.timeline, {
    time: `${replayTimeElapsed(at - tl.start)} / ${replayTimeElapsed(span)}`,
    valueText: replayTimeElapsed(at - tl.start), title: new Date(at).toLocaleTimeString(),
    status: tl.live ? '只读回放' : '会话已结束 · 只读回放',
    bounds: `${tl.start}:${tl.end}`,
    ticks: [0, .25, .5, .75, 1].map(fraction => replayTimeElapsed(span * fraction)),
    playing: !!tl.playing, playLabel: tl.playing ? '暂停' : (tl.atEnd ? '从头播放' : '播放'),
    speed: String(tl.speed), live: !!tl.live,
  });
  publish();
}

function timelineSend(view, message) {
  const ws = view?.ws;
  if (!view?.replay || ws?.readyState !== 1) return false;
  ws.send(JSON.stringify(message));
  return true;
}

function timelineSeekTo(view, unixMs) {
  const tl = view.timeline;
  tl.playing = false;
  tl.clock = unixMs;
  timelineSend(view, { t: 'seek', unix_ms: Math.round(unixMs) });
}

const replayView = () => { const v = currentTermViewObject(); return v?.replay && v.timeline ? v : null; };
const seekTarget = v => v.timeline.start + Number(ui.timeline.seek) / 1000 * Math.max(0, v.timeline.end - v.timeline.start);
function timelinePointerDown() { const v = replayView(); if (v) v.timeline.scrubbing = true; }
function timelineInput(event) {
  const v = replayView(); if (!v) return;
  ui.timeline.seek = event.target.value;
  v.timeline.scrubbing = true; renderTimeline(v);
}
function timelineChange(event) {
  const v = replayView(); if (!v) return;
  ui.timeline.seek = event.target.value;
  v.timeline.scrubbing = false; timelineSeekTo(v, seekTarget(v)); renderTimeline(v);
}
function timelinePointerUp() { setTimeout(() => {
  const v = replayView(); if (v?.timeline.scrubbing) { v.timeline.scrubbing = false; renderTimeline(v); }
}, 0); }
function timelinePointerCancel() { const v = replayView(); if (v) { v.timeline.scrubbing = false; renderTimeline(v); } }
function timelinePlay() {
  const v = replayView(); if (!v) return;
  const tl = v.timeline;
  if (tl.playing) { tl.playing = false; timelineSend(v, {t:'pause'}); }
  else { tl.playing = true; tl.atEnd = false; timelineSend(v, {t:'play', speed:tl.speed}); }
  renderTimeline(v);
}
function timelineSpeed(event) {
  const v = replayView(); if (!v) return;
  v.timeline.speed = Number(event.target.value) || 1;
  if (v.timeline.playing) timelineSend(v, {t:'play', speed:v.timeline.speed});
  renderTimeline(v);
}
function timelineLive() {
  const v = replayView(); if (!v) return;
  v.timeline.playing = false; timelineSend(v, {t:'live'}); renderTimeline(v);
}

function attachRecordingReplay(view, row, uid) {
  let restoreTimeline = view.resumeTimeline;
  view.resumeTimeline = null;
  const name = view.name;
  view.replay = true;
  view.revoked = true;               // 绝不能自动 claim 一个已退出的实例
  view.keepOutput = true;
  cancelTermReconnect(view);
  dropTermSocket(view);
  const url = (HUB_MODE && row.node_id)
    ? new URL(`api/nodes/${encodeURIComponent(row.node_id)}/api/term/records/attach`, APP_BASE)
    : new URL(appUrl('api/term/records/attach'));
  url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
  url.search = new URLSearchParams({id: row.recording.id, mode: view.grid ? 'grid' : 'bytes'});
  const ws = new WebSocket(url.href);
  ws.binaryType = 'arraybuffer';
  view.ws = ws;
  if (T.name === name) T.ws = ws;
  const dec = new TextDecoder();
  view.term.reset();
  view.timeline = { start: 0, end: 0, clock: 0, live: !!row.recording.live, playing: false,
    speed: view.timeline?.speed || 1, atEnd: false, scrubbing: false };
  renderTimeline(view);
  ws.onmessage = e => {
    if (view.ws !== ws) return;
    if (typeof e.data === 'string') {
      let message = null;
      try { message = JSON.parse(e.data); } catch {}
      if (!message) return;
      const tl = view.timeline;
      if (message.t === 'timeline') {
        tl.start = message.start_ms || 0; tl.end = message.end_ms || tl.start; tl.clock = tl.end;
        tl.live = !!message.live;
      } else if (message.t === 'clock') {
        tl.clock = message.unix_ms || tl.clock;
        if (message.end_ms) tl.end = Math.max(tl.end, message.end_ms);
      } else if (message.t === 'record') {
        // 每个 record 帧都是一份完整画面（打开、seek、跨缺口），先清屏再画。
        if (message.cols && message.rows) {
          view.replaySize = { cols: message.cols, rows: message.rows };
          view.term.resize(message.cols, message.rows);
          fitTerm(true);
        }
        if (!view.grid) view.term.reset();
        tl.clock = message.unix_ms || tl.clock;
        tl.atEnd = false;
      } else if (message.t === 'resize' && !view.grid && message.cols && message.rows) {
        view.replaySize = { cols: message.cols, rows: message.rows };
        view.term.resize(message.cols, message.rows);
        fitTerm(true);
      } else if (message.t === 'gap') {
        if (!view.grid) view.term.reset();
      } else if (message.t === 'exit') {
        const code = message.exit?.code;
        ConsoleUI.errors.set(uid, `会话已结束（退出码 ${code}），这是它的录制回放，只读。`);
        renderTakeoverBtn();
      } else if (message.t === 'end') {
        tl.atEnd = true; tl.playing = false;
        // The initial replay opens at its tail. Once that snapshot is complete,
        // restore the position saved when the page detached for sleep/background.
        if (restoreTimeline) {
          const saved = restoreTimeline; restoreTimeline = null;
          timelineSeekTo(view, saved.clock);
          if (saved.playing) {
            tl.playing = true; tl.atEnd = false;
            timelineSend(view, {t:'play', speed:saved.speed});
          }
        }
      }
      renderTimeline(view);
      return;
    }
    writeTermOutput(view, dec.decode(e.data, {stream: true}));
  };
  ws.onclose = () => {
    if (view.ws !== ws) return;
    view.ws = null;
    if (T.name === name) T.ws = null;
    view.ended = true;
    if (!ConsoleUI.errors.get(uid)) ConsoleUI.errors.set(uid, '录制回放结束（只读）。');
    renderTakeoverBtn();
  };
  ws.onerror = () => {
    if (view.ws !== ws) return;
    ConsoleUI.errors.set(uid, '录制回放连接失败。');
    renderTakeoverBtn();
  };
  return true;
}

async function attachTerm(name, auto = false, directClaim = false) {
  if (SessionDockNetwork.paused) return;
  const existing = T.views.get(name);
  if (!await loadTerminalRenderer(name)) return false;
  if (SessionDockNetwork.paused) return false;
  // Loading assets yields: a replaced/disposed host must not be recreated by
  // an old reconnect or theme refresh after the user's next action.
  if (existing && T.views.get(name) !== existing) return false;
  const view = ensureTerm(name);
  if (view.attachPromise) return view.attachPromise;
  const job = attachOwnedTerm(view, true, auto, directClaim).catch(error => {
    if (!auto) throw error;
    if (T.views.get(name) !== view || view.revoked || view.retired || view.ended) return false;
    ConsoleUI.errors.set(T.name === name ? T.uid : view.bindingUid,
      '控制台连接暂时失败，正在自动重试：' + (error.message || error));
    renderTakeoverBtn();
    scheduleTermReconnect(view);
    return false;
  }).finally(() => {
    if (view.attachPromise === job) view.attachPromise = null;
  });
  view.attachPromise = job;
  return job;
}

async function attachOwnedTerm(view, allowRefresh = true, auto = false, directClaim = false) {
  if (SessionDockNetwork.paused) return false;
  if (view.retired) return false;
  if (view.replay && view.ws && view.ws.readyState < 2) return true;
  const name = view.name;
  const wantedUid = view.bindingUid || T.uid;
  const row = (SessionDockCapabilities.config.backend === 'rust'
    ? (String(wantedUid || '').startsWith('tmux:') ? (T.pending || []) : (T.list || []))
    : [...(T.list || []), ...(T.pending || [])]).find(row => row.name === name);
  const uid = row?.uid || T.uid;
  const bound = SessionDockCapabilities.config.backend === 'rust';
  // 进程已退出但有录制：不 claim，直接只读回放录制（会话列表就是录制的索引）。
  // 本页刚观察到的宿主退出（view.ended / pendingPhase）也走这条路，不能
  // 被 ended 提前 return 挡住，否则直播尾帧既没有时间轴也滚不动。
  // 只有启动型（pending）行才有 running 字段；原生会话行是活的，永远走 claim。
  if (bound && row?.recording?.id
      && (view.ended || pendingPhase(row) === 'exited' || pendingPhase(row) === 'failed'))
    return attachRecordingReplay(view, row, uid);
  if (view.ended) return false;
  if (bound && row?.record_id && pendingPhase(row) !== 'running' && pendingPhase(row) !== 'starting' && !row.recording?.id) {
    ConsoleUI.errors.set(uid, row.source === 'shell' ? '会话已结束，没有留下录制。' : '实例已退出。');
    renderTakeoverBtn();
    return false;
  }
  const launch = bound && row?.record_id && row?.launch_id && !row?.stale;
  if (bound && ((!row?.uid && !launch) || !row.instance_id
      || (view.instanceId && view.instanceId !== row.instance_id))) {
    // The pane list and the selected session are refreshed independently. A
    // report dialog (or an SSE update) can leave an already-open view carrying
    // the previous instance for one poll. Refresh the authoritative pane row
    // once before exposing the transient mismatch to the user.
    if (allowRefresh && typeof loadTermList === 'function' && !view.ended && !view.retired) {
      await loadTermList();
      if (T.views.get(name) === view) return attachOwnedTerm(view, false, auto, directClaim);
    }
    ConsoleUI.errors.set(uid, '终端实例关联已失效，请刷新控制台状态。');
    renderTakeoverBtn();
    return false;
  }
  // Capture once: claim and attachment must never silently follow replacement.
  const binding = bound ? (launch
    ? {record_id: row.record_id, launch_id: row.launch_id, instance_id: row.instance_id}
    : {uid: row.uid, instance_id: row.instance_id}) : {};
  if (bound) { view.instanceId = binding.instance_id; view.bindingUid = uid; }
  const active = !!ui.visible && T.name === name;
  if (active) activateTermView(view);
  cancelTermReconnect(view);
  dropTermSocket(view);
  // A fork/current thread can display its ancestor's bound host. Report the
  // attempt on the selected view; the wire binding remains the host's tuple.
  const token = await claimTermOwnership(name, active ? T.uid || uid : uid, binding, auto, directClaim);
  if (SessionDockNetwork.paused) return false;
  if (bound && T.views.get(name) !== view) return false;
  if (!token) {
    view.revoked = true;
    view.focusRequest = null;
    if (T.name === name) closeTermPane();
    return false;
  }
  view.revoked = false;
  // Rust raw HTTP input reuses this exact page lease and binding tuple; the
  // server still decides, so a revoked/expired token simply gets refused.
  view.inputLease = bound ? { token, ...binding } : null;
  dropTermSyncHold(view);
  setCodexSideThreadState(view, false);
  view.ansiTail = '';
  view.selectionLocked = false;
  view.selectionSnapshot = null;
  view.term.reset();
  setTimeout(() => { if (T.name === name) fitTerm(); }, 0);
  const wsUrl = new URL(appUrl('api/term/attach'));
  wsUrl.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const cols = view.term.cols || 120, rows = view.term.rows || termRows();
  const connectionId = globalThis.crypto?.randomUUID?.()
    || `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  view.auditConnectionId = connectionId;
  view.auditConnectStarted = performance.now();
  browserAuditEvent('terminal.connecting', {name, cols, rows, renderer: view.grid ? 'grid' : 'xterm'},
    null, {uid: uid || '', connectionId});
  wsUrl.search = new URLSearchParams({name, page: TERM_PAGE_ID, token,
                                      connection: connectionId, ...binding,
                                      heartbeat: '1',
                                      cols: String(cols), rows: String(rows),
                                      ...(view.grid ? {mode: 'grid'} : {})});
  const ws = new WebSocket(wsUrl);
  ws.binaryType = 'arraybuffer';
  view.ws = ws;
  if (T.name === name) T.ws = ws;
  const dec = new TextDecoder();
  let outputBytes = 0, outputChunks = 0, outputTimer = 0;
  const flushOutputAudit = () => {
    clearTimeout(outputTimer);
    outputTimer = 0;
    if (!outputChunks) return;
    browserAuditEvent('terminal.output_received', {
      name, bytes: outputBytes, chunks: outputChunks,
    }, null, {uid: T.uid || '', connectionId});
    outputBytes = 0;
    outputChunks = 0;
  };
  let settled = false, firstOutput = true;
  ws.onmessage = e => {
    if (view.ws !== ws) return;           // 已替换连接的尾包不能重画新终端
    if (typeof e.data === 'string') {
      let control;
      try { control = JSON.parse(e.data); } catch {}
      if (control?.t === 'heartbeat_ready') {
        startTermHeartbeat(view, ws, uid, connectionId);
        return;
      }
      if (control?.t === 'pong') {
        view.heartbeat?.ack(control.id);
        return;
      }
    }
    if (!settled) {
      // 握手成功不等于接上了会话：服务端在升级之后才 attach，失败时立即以
      // 1011 关闭。只有真正收到会话字节才算连上——此时才清错误、重置退避，
      // 否则坏掉的会话会让按钮每 0.5s 闪一次并无限快速重连。
      settled = true;
      view.reconnectDelay = 500;
      ConsoleUI.errors.delete(uid);
      renderTakeoverBtn();
    }
    if (typeof e.data === 'string') {
      try {
        const message = JSON.parse(e.data);
        if (message?.t === 'revoked') {
          handleTermRevoked(view, message.ip, message.by);
          return;
        }
      } catch { /* 普通终端字符串按原样渲染 */ }
    }
    const s = typeof e.data === 'string' ? e.data : dec.decode(e.data, { stream: true });
    outputBytes += typeof e.data === 'string'
      ? new TextEncoder().encode(e.data).length : e.data.byteLength;
    outputChunks++;
    if (firstOutput) {
      firstOutput = false;
      browserAuditEvent('terminal.first_output', {
        name, bytes: outputBytes, elapsed_ms: Math.round(performance.now() - view.auditConnectStarted),
      }, null, {uid: uid || '', connectionId});
    }
    if (!outputTimer) outputTimer = setTimeout(flushOutputAudit, 750);
    writeTermOutput(view, s);
  };
  ws.onopen = () => {
    if (view.ws !== ws) return;
    cancelTermConnectTimeout(view);
    browserAuditEvent('terminal.opened', {name, cols, rows}, null,
      {uid: T.uid || '', connectionId});
    if (T.name === name) {
      syncTermAliases(view);
      fitTerm(true, true);
      settleActivatedTermView(view);
      setScrollPos(0);
      focusTermIfRequested(view);
    }
  };
  ws.onclose = event => {
    if (view.ws !== ws) return;           // 主动换 socket 后，旧 close 事件作废
    cancelTermConnectTimeout(view);
    cancelTermHeartbeat(view);
    ConsoleUI.errors.set(uid,
      `控制台连接已关闭（WebSocket ${event.code}）${event.reason ? '：' + event.reason : '，服务器未提供详细原因。'}`);
    renderTakeoverBtn();
    writeTermOutput(view, dec.decode());
    flushTermSyncHold(view);
    flushOutputAudit();
    browserAuditEvent('terminal.closed', {
      name, code: event.code, reason: event.reason, clean: event.wasClean,
    }, null, {uid: T.uid || '', connectionId,
      severity: event.code === 1000 ? 'info' : 'warning'});
    view.ws = null;
    view.inputLease = null;
    if (T.name === name) {
      T.ws = null;
    }
    if (event.code === 4001 && event.reason.startsWith('revoked:')) {
      handleTermRevoked(view, event.reason.slice('revoked:'.length));
      return;
    }
    if (SessionDockCapabilities.config.backend === 'rust' && event.code === 4002 && event.reason === 'launch retired') {
      view.revoked = true;
      view.retired = true;
      cancelTermReconnect(view);
      rememberTermOpen(name, false);
      const pending = T.pending.find(row => row.name === name && row.instance_id === view.instanceId);
      const reason = '此启动实例的输入授权已撤销，正在核对取消/退出状态；不会自动重新连接。';
      if (pending) { pending.stale = true; pending.running = false; pending.unavailable_reason = reason; }
      ConsoleUI.errors.set(uid, reason);
      if (pending) { refreshPendingStage(name); void loadTermList(); }
      // The host-performed stop (`session/stop` escalation, `term/kill`)
      // retires the lease before the exit is observed: AI sessions close the
      // pane like a host exit; SSH/shell keeps the retained PTY.
      view.keepOutput = (T.pending || []).some(row => row.name === name && row.source === 'shell')
        || (typeof sessionTerminalFirst === 'function' && sessionTerminalFirst(uid));
      if (T.name === name && !view.keepOutput) closeTermPane();
      const stopNotice = document.querySelector('#session-stop-notice');
      if (uid === S.sel && typeof showSessionStopNotice === 'function'
          && (!stopNotice || stopNotice.hidden)) showSessionStopNotice(reason);
      const wait = document.querySelector('.new-session-wait');
      if (wait && pending && S.sel === pendingUid(name)) wait.textContent = reason;
      renderTakeoverBtn();
      void loadTermList();
      return;
    }
    if (recordHostExit(view, uid, event)) {
      void loadTermList();
      return;
    }
    if (view.revoked) return;
    if (event.code !== 1000 && !settled) {
      // 未收到任何会话字节就断开（例如宿主接不上、attach 失败）。原因写进终端本身，
      // pty 区域直接可见并说明正在重试；按钮不再变灰，也没有拦截用的确认框。
      const why = event.reason || `WebSocket ${event.code}`;
      try { view.term.write(`\r\n\x1b[33m⚠ 控制台连接已关闭：${why}\r\n  正在自动重试…\x1b[0m\r\n`); } catch {}
    }
    // 先刷新 tmux 列表再决定是否重连。若进程刚退出，旧 T.list 仍会短暂把它
    // 判为存活；先排一个重连定时器会向已消失的会话握手，产生 404/close race。
    Promise.resolve(pollLive(true)).finally(() => {
      if (T.views.get(name) === view && !view.ws) scheduleTermReconnect(view);
    });
  };
  ws.onerror = () => {
    if (view.ws !== ws) return;
    ConsoleUI.errors.set(uid, '控制台 WebSocket 连接失败；浏览器未提供更详细的错误，请检查网络或重新连接。');
    renderTakeoverBtn();
    browserAuditEvent('terminal.error', {name}, null,
      {uid: T.uid || '', connectionId, severity: 'error'});
  };
  armTermConnectTimeout(view, ws, uid, connectionId);
  return true;
}

// ---- 滚轮翻历史 ----
let _wheelAcc = 0, _wheelTimer = null, _wheelSeq = 0, _resumeInput = null;
const _wheelRequests = new Set();

function wheelBy(deltaY) {
  _wheelAcc += deltaY;
  if (_wheelTimer) return;
  _wheelTimer = setTimeout(async () => {
    _wheelTimer = null;
    const lines = Math.max(1, Math.min(30, Math.round(Math.abs(_wheelAcc) / 40)));
    const up = _wheelAcc < 0;
    _wheelAcc = 0;
    const seq = ++_wheelSeq;
    const req = post('api/term/scroll', { name: T.name, up, lines });
    _wheelRequests.add(req);
    let d;
    try { d = await req; }
    finally { _wheelRequests.delete(req); }
    if (seq !== _wheelSeq) return;       // 期间已经退出滚动了, 这个响应作废
    if (typeof d.pos === 'number') setScrollPos(d.pos);
  }, 40);
}

/** 退出滚动状态: 连带作废还没发出去和还在路上的滚动请求, 否则它们落地后
 *  会把 tmux 又推回 copy-mode。 */
function abortWheel() {
  _wheelSeq++;
  clearTimeout(_wheelTimer);
  _wheelTimer = null;
  _wheelAcc = 0;
}

function setScrollPos(n) {
  const view = currentTermViewObject();
  if (view) view.scrollPos = n;
}

function cancelTermReconnect(view = currentTermViewObject()) {
  if (!view) return;
  clearTimeout(view.reconnectTimer);
  view.reconnectTimer = null;
}

function cancelTermConnectTimeout(view = currentTermViewObject()) {
  if (!view) return;
  clearTimeout(view.connectTimer);
  view.connectTimer = null;
}

/** 浏览器对 WebSocket 握手没有超时；永久 CONNECTING 必须退出本次租约并重走
 *  现有的存活核对/重连链路，不能据此把独立的 ptyhost 进程判成已退出。 */
function armTermConnectTimeout(view, ws, uid, connectionId) {
  cancelTermConnectTimeout(view);
  view.connectTimer = setTimeout(() => {
    view.connectTimer = null;
    if (view.ws !== ws || ws.readyState !== 0) return;
    view.ws = null;                       // 先作废，随后 close 事件不能重复安排重连
    view.inputLease = null;
    if (T.name === view.name) T.ws = null;
    const reason = '控制台连接建立超时；已中止本次连接并自动重试。';
    ConsoleUI.errors.set(uid, reason);
    renderTakeoverBtn();
    try { view.term.write(`\r\n\x1b[33m⚠ ${reason}\x1b[0m\r\n`); } catch {}
    browserAuditEvent('terminal.connect_timeout', {
      name: view.name, timeout_ms: TERM_CONNECT_TIMEOUT_MS,
    }, null, {uid: uid || '', connectionId, severity: 'warning'});
    try { ws.close(); } catch {}
    Promise.resolve(pollLive(true)).finally(() => {
      if (T.views.get(view.name) === view && !view.ws) scheduleTermReconnect(view);
    });
  }, TERM_CONNECT_TIMEOUT_MS);
}

function dropTermSocket(view = currentTermViewObject()) {
  if (!view) return;
  cancelTermConnectTimeout(view);
  cancelTermHeartbeat(view);
  const ws = view.ws;
  view.ws = null;                        // 先失效引用，close 回调便不会误判成意外断线
  // HTTP keys must not reuse the lease released with this socket. A hidden
  // conversation may stay disconnected and use the verified instance path.
  view.inputLease = null;
  if (T.name === view.name) T.ws = null;
  if (ws) { try { ws.close(); } catch {} }
}

function cancelTermHeartbeat(view) {
  if (!view?.heartbeat) return;
  clearTimeout(view.heartbeat.timer);
  view.heartbeat = null;
}

/** OPEN only describes the local socket. Require a round trip through the node,
 * even when the CLI is quiet or output still arrives on a one-way connection.
 * Wait for the node's opt-in acknowledgement so old nodes never receive probes
 * as literal PTY input. Recovery replaces the transport, never replays keys. */
function startTermHeartbeat(view, ws, uid, connectionId) {
  if (view.heartbeat) return;
  let id = 0, pending = null, sentAt = 0;
  const heartbeat = view.heartbeat = {timer: null, ack(received) {
    if (view.ws !== ws || pending === null || received !== pending) return;
    clearTimeout(heartbeat.timer);
    pending = null;
    heartbeat.timer = setTimeout(ping, TERM_HEARTBEAT_INTERVAL_MS);
  }};
  function ping() {
    if (view.ws !== ws || ws.readyState !== WebSocket.OPEN || document.hidden) return;
    pending = ++id;
    sentAt = performance.now();
    ws.send(JSON.stringify({t: 'ping', id: pending}));
    heartbeat.timer = setTimeout(() => {
      if (view.ws !== ws || pending === null) return;
      browserAuditEvent('terminal.heartbeat_timeout', {
        name: view.name, timeout_ms: TERM_HEARTBEAT_TIMEOUT_MS,
        elapsed_ms: Math.round(performance.now() - sentAt), buffered_bytes: ws.bufferedAmount,
      }, null, {uid: uid || '', connectionId, severity: 'warning'});
      dropTermSocket(view);
      ConsoleUI.errors.set(uid, '控制台连接无响应，正在重新连接；已输入的按键不会自动重发。');
      renderTakeoverBtn();
      scheduleTermReconnect(view);
    }, TERM_HEARTBEAT_TIMEOUT_MS);
  }
  ping();
}

/** 网络短断后自动恢复。tmux 才是会话本体，WebSocket 只是可随时重建的视图。 */
function scheduleTermReconnect(view = currentTermViewObject()) {
  if (SessionDockNetwork.paused) return;
  if (!view || view.revoked || view.retired || view.ended || document.hidden || !navigator.onLine || view.reconnectTimer) return;
  const stillAlive = [...(T.list || []), ...(T.pending || [])].some(x => x.name === view.name);
  if (!stillAlive) return;
  const delay = view.reconnectDelay;
  view.reconnectTimer = setTimeout(() => {
    view.reconnectTimer = null;
    if (T.views.get(view.name) !== view || view.revoked || view.retired || view.ended || document.hidden || !navigator.onLine) return;
    view.reconnectDelay = Math.min(8000, Math.round(view.reconnectDelay * 1.8));
    attachTerm(view.name, true);
  }, delay);
}

/** 手机锁屏会冻结一个看似仍 OPEN、实际已经失效的 socket；恢复时必须强制换新。 */
function reconnectTerm(view = currentTermViewObject()) {
  if (SessionDockNetwork.paused) return;
  if (!view || view.revoked || view.retired || view.ended || document.hidden || !navigator.onLine) return;
  if (view === currentTermViewObject() && !termPaneRenderable(view)) return;
  attachTerm(view.name, true);
  if (T.name === view.name) setTimeout(() => { layoutTermPane(); fitTerm(); }, 20);
}

function suspendTerm() {
  for (const view of T.views.values()) {
    cancelTermReconnect(view);
    dropTermSocket(view);
  }
}

function deactivateTermView() {
  const view = currentTermViewObject();
  if (view) view.host.hidden = true;
  T.name = null;
  syncTermAliases();
  setTermCtrl(false);
  setTermAlt(false);
  setTermShiftSelection(false);
  renderTimeline(null);
  renderTermOutputNotice(null);
}

function disposeTermView(name) {
  const view = T.views.get(name);
  if (!view) return;
  clearTimeout(view.fitFrame);
  if (view.outputLayoutFrame) cancelAnimationFrame(view.outputLayoutFrame);
  const active = T.name === name;
  setCodexSideThreadState(view, false);
  cancelTermReconnect(view);
  dropTermSocket(view);
  try { view.term.dispose(); } catch { /* 已被浏览器清理 */ }
  view.menu?.dispose();
  view.host.remove();
  T.views.delete(name);
  if (active) {
    deactivateTermView();
    ui.visible = false; publish();
    ui.collapsed = false; publish();
    $('#right').classList.remove('term-full');
  }
}

// ---------------------------------------------------------------- 输入框
// 已接管的会话在消息流底部给个输入框, 不必展开整个终端就能说话。

function setTermShiftSelection(on) { T.shiftSelect = ui.shiftSelect = !!on; publish(); }
function setTermAlt(on) { T.altArmed = ui.altArmed = !!on; publish(); }

/** Match physical Alt: CSI modifier for navigation, ESC prefix for a character.
 * Ignore paste, mouse, focus and terminal replies; they are not the next key. */
function applyTermAlt(data) {
  if (!T.altArmed) return data;
  const cursor = /^\x1b(?:\[|O)([ABCDHF])$/.exec(data);
  const modified = /^\x1b\[(\d+);(\d+)([ABCDHF~])$/.exec(data);
  const page = /^\x1b\[(\d+)~$/.exec(data);
  let result;
  if (cursor) result = `\x1b[1;3${cursor[1]}`;
  else if (modified) result = `\x1b[${modified[1]};${1 + ((Number(modified[2]) - 1) | 2)}${modified[3]}`;
  else if (page && ['2', '3', '5', '6'].includes(page[1])) result = `\x1b[${page[1]};3~`;
  else if (data.length === 1) result = '\x1b' + data;
  else return data;
  setTermAlt(false);
  return result;
}

function setTermCtrl(on) { T.ctrlArmed = ui.ctrlArmed = !!on; publish(); }

/** 把 Ctrl 修饰的单个 ASCII 键转换为终端控制字节。多字符粘贴和中文不改写。 */
function applyTermCtrl(data) {
  if (!T.ctrlArmed) return data;
  // focus-events、鼠标和粘贴都会产生多字节数据，它们不是用户要修饰的“下一键”。
  if (data.length !== 1) return data;
  setTermCtrl(false);
  const code = data.charCodeAt(0);
  if ((code >= 64 && code <= 95) || (code >= 97 && code <= 122)) {
    return String.fromCharCode(code & 31);
  }
  const aliases = { ' ': 0, '2': 0, '3': 27, '4': 28, '5': 29, '6': 30, '7': 31, '8': 127, '?': 127 };
  return Object.hasOwn(aliases, data) ? String.fromCharCode(aliases[data]) : data;
}

function shortcutClick(e) {
  const modifier = e.target.closest('[data-term-modifier]');
  if (modifier) {
    if (modifier.dataset.termModifier === 'shift') {
      setTermShiftSelection(!T.shiftSelect);
      return; // Selecting output must not focus the CLI or raise its keyboard.
    }
    if (modifier.dataset.termModifier === 'alt') setTermAlt(!T.altArmed);
    else setTermCtrl(!T.ctrlArmed);
    T.term?.focus();
    return;
  }
  const b = e.target.closest('[data-term-key]');
  if (!b) return;
  let key = T.ctrlArmed ? `C-${b.dataset.termKey}` : b.dataset.termKey;
  if (T.altArmed) {
    const navigation = {Up:'A', Down:'B', Right:'C', Left:'D'};
    const final = navigation[b.dataset.termKey];
    const m = T.ctrlArmed ? 7 : 3;
    if (final) key = `\x1b[1;${m}${final}`;
    else if (['PPage', 'NPage'].includes(b.dataset.termKey)) {
      key = `\x1b[${b.dataset.termKey === 'PPage' ? 5 : 6};${m}~`;
    } else key = `M-${key}`;
  }
  setTermAlt(false);
  setTermCtrl(false);
  sendToSession(null, [key]);
  T.term?.focus();
};

// ---- 高度拖动 ----
let tdrag = false;
let tdragPointer = null;
let tdragTopSnap = 48;
function startTermDrag(e) {
  tdrag = true;
  tdragPointer = e.pointerId;
  const composer = $('#composer');
  tdragTopSnap = Math.max(48, composer.offsetHeight || 0);
  e.currentTarget.setPointerCapture?.(e.pointerId);
  document.body.classList.add('dragging-v');
  e.preventDefault();
}
document.addEventListener('pointermove', e => {
  if (!tdrag || e.pointerId !== tdragPointer) return;
  const right = $('#right');
  const top = right.getBoundingClientRect().top;
  const y = Math.max(0, Math.min(right.clientHeight, e.clientY - top));
  const h = right.clientHeight - y;
  if (h <= 32) {
    T.mode = 'collapsed';
  } else if (y <= tdragTopSnap) {
    T.mode = 'full';
  } else {
    T.mode = 'normal';
    T.height = Math.round(h);
  }
  layoutTermPane();
});
function finishTermDrag(e) {
  if (!tdrag || e.pointerId !== tdragPointer) return;
  tdrag = false;
  tdragPointer = null;
  document.body.classList.remove('dragging-v');
  store.set('termh', T.height);
  store.set('termmode', T.mode);
  rememberTermLayout();
  renderTakeoverBtn();
  fitTerm();
}
document.addEventListener('pointerup', finishTermDrag);
document.addEventListener('pointercancel', finishTermDrag);

// 移动浏览器锁屏后常保留一个 readyState=OPEN 的僵尸 WebSocket。进入后台时主动
// 放弃这条传输，回到前台/pageshow/网络恢复时重新 attach；tmux 进程不会受影响。
let termWasBackgrounded = false;
function backgroundTerm() {
  termWasBackgrounded = true;
  for (const view of T.views.values()) {
    view.resumeFocus = !!document.activeElement && view.host.contains(document.activeElement);
    if (view.replay && view.ws && !view.resumeTimeline) view.resumeTimeline = {...view.timeline};
  }
  suspendTerm();
}
function foregroundTerm(force = false) {
  if (SessionDockNetwork.paused) return;
  if (document.hidden || (!force && !termWasBackgrounded)) return;
  termWasBackgrounded = false;
  for (const view of T.views.values()) {
    if (view.webgl) {
      try { view.term.clearTextureAtlas(); } catch {}
    }
    if (view.resumeFocus) requestTermFocus(view, document.body);
    view.resumeFocus = false;
    if (view.replay && view.resumeTimeline) void attachTerm(view.name, true);
    else reconnectTerm(view);
  }
}
document.addEventListener('visibilitychange', () => {
  if (document.hidden) backgroundTerm();
  else { foregroundTerm(); pollComposerInput(); if (bridge.composer.uid) followServerDraft(bridge.composer.uid); }
});
addEventListener('focus', () => { pollComposerInput(); if (bridge.composer.uid) followServerDraft(bridge.composer.uid); });
addEventListener('pagehide', backgroundTerm);
addEventListener('pageshow', e => foregroundTerm(e.persisted));
addEventListener('online', () => foregroundTerm(true));
addEventListener('sessiondock-network-paused', backgroundTerm);
addEventListener('sessiondock-network-resumed', () => {
  foregroundTerm(true);
  void loadTermList();
  void recoverComposerDrafts();
  void pollComposerInput();
  if (bridge.composer.uid) void followServerDraft(bridge.composer.uid);
});

// Native/global process discovery and managed terminal transport are independent
// Rust capabilities. The live poll normally refreshes this list; do not
// lose discovery of new/replacement hosts just because Rust keeps live:false.
async function pollRustTermList() {
  if (SessionDockCapabilities.config.backend !== 'rust'
      || !SessionDockCapabilities.allows('terminal') || SessionDockCapabilities.allows('live')) return;
  try {
    if (!document.hidden) await loadTermList();
  } catch { /* Keep the next observation available after a render/network error. */ }
  finally { setTimeout(pollRustTermList, 3000); }
}

function start() {
  loadTermList();
  if (SessionDockCapabilities.config.backend === 'rust'
      && SessionDockCapabilities.allows('terminal') && !SessionDockCapabilities.allows('live'))
    setTimeout(pollRustTermList, 3000);
}

return { T, TERM_PAGE_ID, termRows, termTheme, configuredTermFont, termFont, termFontSize, terminalFontGridRatio, prepareTerminalFont, hexToRgb, hslLightness, reflectedLightRgb, indexedTerminalRgb, adaptRgbForLight, adaptColonRgb, adaptSgrBody, stripOscColorSets, stripOscColorReports, lightTerminalAnsi, terminalColorChunk, refreshTerminalPreferences, terminalListUncertain, mergeUnavailableTermRows, loadTermList, fetchTermList, sessionTermMeta, termBindingServes, linkedTermSession, takenOver, termSendLease, termRowBinding, termInputBody, adoptLinkedTermSession, rebindSelectedTermSession, toggleLinkedTermSession, takeover, workerStatusMessage, pendingPhase, pendingStateLabel, pendingStageMessage, pendingRecordMissing, notePendingInput, pendingSessionRow, pendingShellRunning, pendingTitle, refreshPendingStage, notePendingEnded, pendingSelectionGone, stopPendingSession, deletePendingSession, discardPendingSession, sessionIsPtyOnly, sessionTerminalFirst, openPendingSession, discardAbandonedNewSession, resolveNewSession, auditTermPane, currentTermViewObject, syncTermAliases, activateTermView, requestTermFocus, focusTermIfRequested, legacyCopyText, copyTermSelection, decodeOsc52Clipboard, handleOsc52Clipboard, rememberTermSelection, restoreTermSelection, termSelectionMouseDown, shouldUseTermWebgl, consoleRendererFor, consoleRendererIsGrid, ensureTerm, writeTermOutput, termSyncFrameOpen, flushTermSyncHold, terminalViewportHasCodexSideThread, codexSideThreadVisible, setCodexSideThreadState, scheduleCodexSideThreadScan, writeParsedTermOutput, positionTermViewport, dropTermSyncHold, termPaneRenderable, repaintTermView, performTermFit, refreshTerminalScale, fitTerm, settleActivatedTermView, currentTermView, rememberTermLayout, rememberTermOpen, restoreTermPane, loadTerminalRenderer, openTermPane, toggleTermPane, revealConversationForPrompt, closeTermPane, layoutTermPane, claimTermOwnership, describeTermTaker, handleTermRevoked, renderTermOutputNotice, recordHostExit, startShellRecordingReplay, sessionRecordingReplayable, replayTimeElapsed, renderTimeline, timelineSend, timelineSeekTo, timelinePointerDown, timelineInput, timelineChange, timelinePointerUp, timelinePointerCancel, timelinePlay, timelineSpeed, timelineLive, attachRecordingReplay, attachTerm, attachOwnedTerm, wheelBy, abortWheel, setScrollPos, cancelTermReconnect, cancelTermConnectTimeout, armTermConnectTimeout, dropTermSocket, cancelTermHeartbeat, startTermHeartbeat, scheduleTermReconnect, reconnectTerm, suspendTerm, deactivateTermView, disposeTermView, setTermShiftSelection, setTermAlt, applyTermAlt, setTermCtrl, applyTermCtrl, shortcutClick, startTermDrag, finishTermDrag, backgroundTerm, foregroundTerm, pollRustTermList, start };
}
