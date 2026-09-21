'use strict';

// 接管会话: 在服务端把它用 tmux resume 起来, 然后把终端嵌在会话详情底部。
// SSH 例外：PTY 在输入框上方。
// 会话跑在 tmux 里, 所以关掉页面/重启 sessiondock 都不会打断它。
// Hub 的节点侧连接/响应上限是 10 秒；再留出反向代理与浏览器调度余量。
// WebSocket 没有标准的建立超时，必须由页面回收永久 CONNECTING 的尝试。
const TERM_CONNECT_TIMEOUT_MS = 15_000;
const TERM_CLAIM_TIMEOUT_MS = 5_000;
// DEC 2026 同步帧在页面这层暂存的上限：超过就先交给 xterm（它自己对 2026 还有
// 1 s 兜底），不让一个没收尾的帧无限占住输出。
const TERM_SYNC_HOLD_MAX = 256 * 1024;
const TERM_SYNC_HOLD_MS = 100;
const TERM_LAYOUT_POLICY_VERSION = 2;
// 每次页面加载独立生成；不写 local/sessionStorage，复制标签页也不会复制归属。
const TERM_PAGE_ID = window.__sessiondockPageId || crypto.randomUUID?.()
  || [...crypto.getRandomValues(new Uint8Array(16))]
    .map(value => value.toString(16).padStart(2, '0')).join('');

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
  backend: '',     // 本机当前的终端后端；hub 模式下按机器看 Nodes.capabilities
  backends: [],
  height: store.get('termh', 320),
  mode: store.get('termmode', 'full'), // normal(手动分屏) | collapsed(对话：PTY+输入) | full(纯终端)
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
// Claude 双 Esc 的最终叶子有时只保存在 TUI 进程内，不会追加 JSONL。
// 以 tmux 名为键跟踪原生选择器，确认后再让服务端从当前屏幕同步时间线。
const claudeRewinds = new Map();

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
  for (const name of redrawNames) attachTerm(name);
  setTimeout(() => { fitTerm(); }, 0);
}

// xterm 先测量网页字体再创建 DOM 行，避免按回退字体计算出错误的字符宽度。
let terminalFontReady = prepareTerminalFont();
addEventListener('resize', () => { layoutTermPane(); fitTerm(); });

let termListRequestSeq = 0;
function loadTermList() {
  const request = fetchTermList().finally(() => { if (T.listRequest === request) T.listRequest = null; });
  T.listRequest = request;
  return request;
}
async function fetchTermList() {
  const requestSeq = ++termListRequestSeq;
  const openEpoch = termOpenEpoch;
  const fingerprint = () => [
    ...(T.list || []).map(x => `${x.name}\t${x.cwd}` + (SessionDockCapabilities.config.backend === 'rust' ? `\t${x.uid}\t${x.instance_id}\t${x.current_uid || ''}` : '')),
    ...(T.pending || []).map(x => `pending\t${x.name}\t${x.cwd}` + (SessionDockCapabilities.config.backend === 'rust' ? `\t${x.record_id}\t${x.instance_id}\t${x.state}` : '')),
  ].join('\n');
  const before = fingerprint();
  let loaded = false;
  let data = null;
  let failure = '';
  let transient = false;   // Rust：网络错误 / 5xx / 429 只影响这一轮，不清空控制台状态
  try {
    const response = await fetch(appUrl('api/term/list'));
    transient = response.status === 429 || response.status >= 500;
    data = await response.json();
    if (!response.ok || data.error) throw new Error(
      `终端列表请求失败（HTTP ${response.status}）：${data.error || response.statusText}`);
    loaded = true;
    transient = false;
  } catch (error) {
    failure = error.message || String(error);
    if (error?.name === 'TypeError') transient = true;
  }
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
    T.list = data.sessions || [];
    T.sources = data.sources || {};
    T.resume_sources = data.resume_sources || {};
    T.home = data.home || '';
    T.backend = data.backend || '';
    T.backends = data.backends || [];
    const hadSelected = String(S.sel || '').startsWith('tmux:')
      && (T.pending || []).some(row => pendingUid(row.name) === S.sel);
    T.pending = data.pending || [];
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
    T.backends = [];
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
        && ((T.name === name && !$('#termpane').classList.contains('hidden'))
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
        && !$('#termpane')?.classList.contains('hidden')
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
      || (!SessionDockCapabilities.allows('outbox') && SessionDockCapabilities.config.conversation_send!==true)) return {};
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
      const ok = confirm(
        `这个会话正在运行中（${n} 个进程），而且不在 tmux 里，无法直接接入。\n\n`
        + `接管会先结束正在运行的实例，再用 tmux 重新打开它。\n`
        + `未保存的输入会丢失，已完成的对话不受影响。\n\n继续吗？`);
      if (!ok) return;
      setBtn('结束旧实例…', true);
      d = await post('api/term/takeover', { uid, force: true, cols: 120, rows: termRows() });
    }
    if (d.error) {
      ConsoleUI.errors.set(uid, d.error);
      return alert('打开控制台失败：' + d.error);
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

async function post(url, body, {timeoutMs = 0} = {}) {
  const traceId = String(body?.request_id || globalThis.crypto?.randomUUID?.()
    || `${Date.now()}-${Math.random().toString(36).slice(2)}`);
  const payload = { ...body, _build: BUILD_ID, _trace_id: traceId,
    _page_id: TERM_PAGE_ID };
  browserAuditEvent?.('http.request.started', {url, method: 'POST'}, null, {
    uid: body?.uid || '', traceId, requestId: body?.request_id || '',
  });
  const started = performance.now();
  let phase = 'headers', headersMs = null, status = null;
  const controller = timeoutMs ? new AbortController() : null;
  const timer = controller ? setTimeout(() => controller.abort(), timeoutMs) : null;
  try {
    const r = await fetch(appUrl(url), {
      method: 'POST', headers: {
        'Content-Type': 'application/json', 'X-SessionDock-Trace': traceId,
        'X-SessionDock-Page': TERM_PAGE_ID, 'X-SessionDock-Build': BUILD_ID,
      },
      body: JSON.stringify(payload),
      ...(controller ? {signal: controller.signal} : {}),
    });
    headersMs = Math.round((performance.now() - started) * 1000) / 1000;
    status = r.status;
    phase = 'body';
    browserAuditEvent?.('http.response.headers', {url, status, headers_ms: headersMs}, null,
      {uid: body?.uid || '', traceId, requestId: body?.request_id || ''});
    const text = await r.text();
    const bodyMs = Math.round((performance.now() - started - headersMs) * 1000) / 1000;
    phase = 'parse';
    let data;
    try {
      data = JSON.parse(text);
    } catch {
      throw new Error(r.status >= 500
        ? '服务暂时不可用，请稍后重试'
        : '服务返回了无法解析的响应，请重新加载');
    }
    browserAuditEvent?.('http.response.received', {
      url, status: r.status, ok: r.ok, headers_ms: headersMs,
      body_ms: bodyMs,
      duration_ms: Math.round((performance.now() - started) * 1000) / 1000,
    }, data, {uid: body?.uid || '', traceId, requestId: body?.request_id || '',
      severity: r.ok ? 'info' : 'warning'});
    if (data?.reload) markStaleBuild(data.build);
    return data;
  } catch (error) {
    if (controller?.signal.aborted) {
      error = new Error('请求超时，服务端可能已执行；请重新核对状态。');
      error.name = 'TimeoutError';
    }
    browserAuditEvent?.('http.request.failed', {
      url, error: String(error?.stack || error), phase, status, headers_ms: headersMs,
      timeout_ms: timeoutMs, online: navigator.onLine, visibility: document.visibilityState,
      duration_ms: Math.round((performance.now() - started) * 1000) / 1000,
    }, null, {uid: body?.uid || '', traceId, requestId: body?.request_id || '',
      severity: 'error'});
    throw error;
  } finally {
    if (timer !== null) clearTimeout(timer);
  }
}

// ---------------------------------------------------------------- 缺陷报告
let bugReportToastTimer = 0;

const BUG_REPORT_SOURCES = { claude: 'Claude', codex: 'Codex', grok: 'Grok' };

function bugReportSource() {
  return $('#bug-report-source input:checked')?.value || 'codex';
}

// 与新建会话一样，三种 CLI 都可以做处理会话；记住上次的选择，处理机器上
// 缺少的命令置灰（中央站下按所选机器的能力表，单机按本机）。
function syncBugReportSources() {
  const remembered = store.get('bugReportSource', 'codex');
  const sources = HUB_MODE ? Nodes.capabilities[bugReportNode()]?.sources : T.sources;
  const known = sources && Object.keys(sources).length;
  let checked = null;
  for (const input of $('#bug-report-source').querySelectorAll('input')) {
    const missing = known && !sources[input.value];
    input.disabled = !!missing;
    input.title = missing ? `${bugReportNodeName() || '本机'}找不到 ${input.value} 命令` : '';
    if (input.value === remembered && !missing) checked = input;
  }
  const fallback = checked || [...$('#bug-report-source').querySelectorAll('input')]
    .find(input => !input.disabled);
  if (fallback) fallback.checked = true;
}

// 处理会话的机器下拉与新建会话一样列出全部机器；默认选问题所在的机器
// （当前会话的机器），其次上次的选择，再次唯一筛选中的机器。
function prepareBugReportNode() {
  if (!HUB_MODE) return;
  const select = $('#bug-report-node');
  const origin = bugReportOriginNode();
  const selected = selectedNodeIds();
  const preferred = [origin, store.get('bugReportNode', ''),
    selected.length === 1 ? selected[0] : ''].filter(Boolean);
  select.replaceChildren();
  for (const n of Nodes.list) {
    const option = document.createElement('option');
    option.value = n.id;
    option.textContent = n.name + (Nodes.capabilities[n.id]?.enabled ? '' : '（离线或未启用终端）');
    option.disabled = !Nodes.capabilities[n.id]?.enabled;
    select.appendChild(option);
  }
  const usable = id => [...select.options].some(o => o.value === id && !o.disabled);
  select.value = preferred.find(usable) || [...select.options].find(o => !o.disabled)?.value || '';
  $('#bug-report-node-label').hidden = false;
}

function bugReportNodeName(id = bugReportNode()) {
  return HUB_MODE ? Nodes.list.find(n => n.id === id)?.name || '' : '';
}

function showBugReportToast(report, worker) {
  const toast = $('#bug-report-toast');
  clearTimeout(bugReportToastTimer);
  toast.replaceChildren();
  const text = document.createElement('span');
  const label = BUG_REPORT_SOURCES[worker?.source] || '处理';
  const where = worker?.node_name ? `到 ${worker.node_name}` : '';
  text.textContent = `${report} 已保存${where}，${label} 处理会话正在启动`;
  const open = document.createElement('button');
  open.type = 'button';
  open.className = 'btn';
  open.textContent = '打开';
  open.onclick = async () => {
    toast.classList.add('hidden');
    await loadTermList();
    const pending = (T.pending || []).find(item => item.name === worker.name) || worker;
    await openPendingSession(pending);
  };
  toast.append(text, open);
  toast.classList.remove('hidden');
  bugReportToastTimer = setTimeout(() => toast.classList.add('hidden'), 20000);
}

// 报告框的附件复用对话输入框那一套：同样的选择菜单、粘贴/拖放、[附件N]
// 引用，以及同一个上传接口。处理会话的 cwd 固定为仓库根目录，因此上传
// 先流式暂存到服务端私有目录，点击发送后才发布到仓库 sessiondock_attachments/。
// newComposerDraft 定义在下方的对话输入框段落，只能在运行时按需创建。
let BUG_REPORT_DRAFT_UID = '';
function bindBugReportDraft() {
  const node=bugReportNode();
  const key='reportDraftId.' + node;
  let id=store.get(key,'');
  if (!id) {id=crypto.randomUUID(); store.set(key,id);}
  BUG_REPORT_DRAFT_UID='report:' + (HUB_MODE ? node + '~' : '') + id;
}
let bugReportSending = false;
const bugReportDraftObject = () => composerDraft(BUG_REPORT_DRAFT_UID);

/** 下拉选的是跑处理会话的机器，不是另一份报告：正在写的描述、引用和附件
 *  跟着这次选择走，切换机器不清空输入框。草稿本身仍然一机一份（附件的字节
 *  暂存在处理机器上），所以这里把内容搬到新机器的草稿上，再把原机器那份清
 *  空；目标机器上已有的服务端草稿由 hydrate 的早期编辑合并规则接上，两边
 *  都不丢。本页仍握着 File 的附件在新机器上重新暂存，原机器的暂存字节随
 *  清空后的草稿释放；重开页面后只剩服务端引用的卡片没有字节可重传，留在
 *  原机器的草稿里并在对话框上说明。返回要显示的提示文案。 */
function carryBugReportDraft(fromUid, toUid) {
  if (!fromUid || !toUid || fromUid === toUid || bugReportSending) return '';
  const from = composerDrafts.get(composerDraftOwner(fromUid));
  if (!from || from.handedOffSession) return '';
  const moving = from.attachments.filter(item => item.file instanceof Blob);
  const stranded = from.attachments.length - moving.length;
  if (!from.text && !from.quotes.length && !moving.length) return '';
  const to = composerDraft(toUid);
  if (!to) return '';
  const node = bugReportNode();
  to.text = to.text ? to.text + '\n' + from.text : from.text;
  to.quotes = [...to.quotes, ...from.quotes];
  to.attachments = [...to.attachments, ...moving];
  to.nextAttachmentNumber = Math.max(to.nextAttachmentNumber || 1, from.nextAttachmentNumber || 1);
  ensureComposerAttachmentNumbers(to);
  for (const field of ['requestId','requestText','report_prompt','report_text']) {
    delete to[field]; delete from[field];
  }
  from.attachments = from.attachments.filter(item => !moving.includes(item));
  from.text = ''; from.quotes = [];
  if (!from.attachments.length) from.nextAttachmentNumber = 1;
  const saved = persistComposerDraft(fromUid);
  for (const item of moving) {
    // 原机器上的暂存字节随清空后的草稿释放；新机器要的是一份新的上传。
    const previous = item.uploaded;
    if (item.cancelUpload) item.cancelUpload();
    item.uploaded = null; item.status = ''; item.error = '';
    discardStagedAttachment({uploaded: previous}, saved);
    const restage = () => {
      if (!to.attachments.includes(item)) return;
      // 被取消前已经落地的上传仍会写回 uploaded，再清一次才会重新暂存。
      if (item.uploaded) discardStagedAttachment(item, saved);
      item.uploaded = null; item.status = ''; item.error = '';
      stageComposerAttachment(item, toUid, {node, render: renderBugReportItems});
    };
    (item.staging || Promise.resolve()).then(restage, restage);
  }
  persistComposerDraft(toUid);
  if (!stranded) return '';
  const where = bugReportNodeName(nodeOf(fromUid)) || '原机器';
  return `${stranded} 个附件的文件只暂存在${where}，已留在那台机器的草稿里；`
    + '要随这份报告一起提交，请重新选择文件。';
}

function renderBugReportItems() {
  renderAttachmentCards($('#bug-report-items'), bugReportDraftObject().attachments, {
    uid: BUG_REPORT_DRAFT_UID, render: renderBugReportItems,
    disabled: bugReportSending,
    onInsert: number => insertComposerReference(number, $('#bug-report-description')),
    onRetry: attachment => stageComposerAttachment(attachment, BUG_REPORT_DRAFT_UID,
      {node: bugReportNode(), render: renderBugReportItems}),
    onRemove: id => {
      if (bugReportSending) return;
      const removed = removeDraftAttachment(bugReportDraftObject(), id);
      discardStagedAttachment(removed, persistComposerDraft(BUG_REPORT_DRAFT_UID));
      renderBugReportItems();
    },
  });
  renderSavedComposerInputs($('#bug-report-items'), bugReportDraftObject(), BUG_REPORT_DRAFT_UID);
  $('#bug-report-description').disabled = bugReportSending || !!bugReportDraftObject().loading;
  $('#bug-report-add').disabled = bugReportSending || !!bugReportDraftObject().loading;
  // 发送中换机器会把正在提交的内容搬走；锁住下拉直到这一次提交结束。
  $('#bug-report-node').disabled = bugReportSending;
}

function addBugReportFiles(files) {
  const draft = bugReportDraftObject();
  const before = new Set(draft.attachments);
  addDraftFiles(draft, files);
  persistComposerDraft(BUG_REPORT_DRAFT_UID);
  for (const attachment of draft.attachments) {
    if (!before.has(attachment)) {
      stageComposerAttachment(attachment, BUG_REPORT_DRAFT_UID, {node: bugReportNode(), render: renderBugReportItems});
    }
  }
  renderBugReportItems();
}

function clearBugReportDraft() {
  const draft=bugReportDraftObject(), dropped=draft.attachments;
  for (const attachment of dropped) {
    if (attachment.preview) URL.revokeObjectURL(attachment.preview);
    if (attachment.cancelUpload) attachment.cancelUpload();
  }
  draft.text='';draft.attachments=[];draft.quotes=[];draft.nextAttachmentNumber=1;
  delete draft.requestId;delete draft.requestText;
  const saved=persistComposerDraft(BUG_REPORT_DRAFT_UID);
  for (const attachment of dropped) discardStagedAttachment(attachment, saved);
  renderBugReportItems();
}

// 处理会话（以及报告、附件）落在下拉里选中的机器上。
function bugReportNode() {
  return HUB_MODE ? $('#bug-report-node').value || '' : '';
}

// 问题所在的机器：当前会话的机器；没有会话时取唯一筛选中的机器；筛选着
// 多台机器又没选会话时说不清是哪台，按处理机器本身算。
function bugReportOriginNode() {
  if (!HUB_MODE) return '';
  const fromSession = nodeOf(S.sel);
  if (fromSession) return fromSession;
  const candidates = selectedNodeIds();
  return candidates.length === 1 ? candidates[0] : '';
}

// 报告里注明的问题机器；单机模式由服务端填主机名。
function bugReportOrigin(workerNode) {
  const uid = S.sel || '';
  if (!HUB_MODE) return {uid};
  const nodeId = bugReportOriginNode() || workerNode;
  return {node_id: nodeId, node_name: bugReportNodeName(nodeId), uid};
}

function bugReportNodeError(node) {
  if (!HUB_MODE) return '';
  if (!node) return '没有可用的机器：请先在顶部选择一台在线机器或打开一个会话';
  const info = Nodes.list.find(n => n.id === node);
  if (info?.online === false) {
    return `${info.name || '所选机器'} 离线，无法在该机器上保存报告；请换一台在线机器`;
  }
  if (!Nodes.capabilities[node]?.enabled) {
    return `${info?.name || '所选机器'} 未启用终端，无法启动处理会话；请换一台机器`;
  }
  return '';
}

// 处理机器不是问题机器时，先向问题机器要一份服务端上下文（会话行、发送账本、
// 终端画面、审计窗口），随报告交给处理机器；问题机器离线或抓取失败时不阻断
// 报告，只把原因写进诊断包。
async function captureBugReportContext(originNode, uid, terminalName) {
  try {
    const d = await post('api/bug-report/capture', {
      _node: originNode, uid, terminal_name: terminalName, page_id: TERM_PAGE_ID,
    });
    if (d.error) return {error: d.error};
    return d;
  } catch (failure) {
    return {error: `抓取失败：${failure.message || failure}`};
  }
}

function closeBugReportAttachMenu() {
  $('#bug-report-attach-menu').classList.add('hidden');
  $('#bug-report-form .report-composer').style.removeProperty('padding-top');
  $('#bug-report-add').classList.remove('on');
  $('#bug-report-add').setAttribute('aria-expanded', 'false');
}

function openBugReportDialog() {
  const dialog = $('#bug-report-dialog');
  $('#bug-report-error').textContent = '';
  $('#bug-report-go').disabled = false;
  $('#bug-report-go').textContent = '发送';
  setSendButtonBusy($('#bug-report-go'), '');
  prepareBugReportNode();
  bindBugReportDraft();
  renderBugReportItems();
  $('#bug-report-description').value = bugReportDraftObject().text;
  hydrateComposerDraft(BUG_REPORT_DRAFT_UID).then(async () => {
    const handedOff=bugReportDraftObject().handedOffSession;
    if (handedOff) {
      showBugReportToast(handedOff.report_id,handedOff);
      store.set('reportDraftId.'+bugReportNode(),crypto.randomUUID());bindBugReportDraft();
      await hydrateComposerDraft(BUG_REPORT_DRAFT_UID);
    }
    if (!bugReportSending) {
      $('#bug-report-description').value = bugReportDraftObject().text;
      renderBugReportItems();
      autoGrow($('#bug-report-description'));
    }
  });
  syncBugReportSources();
  dialog.showModal();
  closeBugReportAttachMenu();
  autoGrow($('#bug-report-description'));
  setTimeout(() => $('#bug-report-description').focus(), 0);
}

// 详情标题栏是动态生成的，使用委托让列表页、普通会话和尚未落盘的
// 新会话共用同一个入口；手机进入详情后列表顶栏会被完整隐藏。
document.addEventListener('click', event => {
  if (!event.target.closest('[data-report-bug]')) return;
  openBugReportDialog();
});
$('#bug-report-node').onchange = () => {
  const previous = BUG_REPORT_DRAFT_UID;
  store.set('bugReportNode', bugReportNode());
  bindBugReportDraft();
  const notice = carryBugReportDraft(previous, BUG_REPORT_DRAFT_UID);
  $('#bug-report-description').value=bugReportDraftObject().text;
  renderBugReportItems();
  hydrateComposerDraft(BUG_REPORT_DRAFT_UID);
  syncBugReportSources();
  $('#bug-report-error').textContent = notice;
  autoGrow($('#bug-report-description'));
};
$('#bug-report-source').addEventListener('change', () => store.set('bugReportSource', bugReportSource()));
$('#bug-report-dialog .modal-close').onclick = () => $('#bug-report-dialog').close();
$('#bug-report-description').addEventListener('input', event => {
  bugReportDraftObject().text = event.target.value;
  persistComposerDraft(BUG_REPORT_DRAFT_UID);
  autoGrow(event.target);
});
window.addEventListener('resize', () => {
  if ($('#bug-report-dialog').open) autoGrow($('#bug-report-description'));
});
$('#bug-report-dialog').addEventListener('click', event => {
  if (event.target === $('#bug-report-dialog')) $('#bug-report-dialog').close();
});
$('#bug-report-add').onclick = event => {
  event.stopPropagation();
  const menu = $('#bug-report-attach-menu');
  const open = menu.classList.toggle('hidden');
  $('#bug-report-add').classList.toggle('on', !open);
  $('#bug-report-add').setAttribute('aria-expanded', String(!open));
  const row = $('#bug-report-form .report-composer');
  if (open) row.style.removeProperty('padding-top');
  else {
    row.style.paddingTop = `${menu.offsetHeight + 7}px`;
    $('#bug-report-add').scrollIntoView({block: 'nearest'});
  }
};
$('#bug-report-attach-menu').onclick = event => {
  const button = event.target.closest('button[data-attach]');
  if (!button) return;
  closeBugReportAttachMenu();
  const input = $('#bug-report-file');
  input.accept = ATTACH_ACCEPT[button.dataset.attach] ?? '';
  input.click();
};
$('#bug-report-file').onchange = event => {
  addBugReportFiles([...event.target.files]);
  event.target.value = '';
};
$('#bug-report-dialog').addEventListener('click', event => {
  if (!event.target.closest('#bug-report-dialog .attach-picker')) closeBugReportAttachMenu();
});
// 截图通常来自系统剪贴板；粘贴落在描述框或对话框内任意位置都接收。
$('#bug-report-form').addEventListener('paste', event => pasteAttachmentFiles(event, addBugReportFiles));
bindFileDrop($('#bug-report-form'), addBugReportFiles);

function setSendButtonBusy(button, label) {
  if (!button) return;
  button.setAttribute('aria-busy', label ? 'true' : 'false');
  if (label) button.setAttribute('aria-label', label);
  else button.removeAttribute('aria-label');
}

function completeBugReportSubmission(data,node) {
  const oldUid=BUG_REPORT_DRAFT_UID,draft=composerDrafts.get(oldUid);
  if (draft) {
    for (const item of draft.attachments) if (item.preview) URL.revokeObjectURL(item.preview);
    composerSaveQueues.delete(draft);composerPendingSaves.delete(draft);composerSaving.delete(draft);
  }
  composerDrafts.delete(oldUid);composerHydrations.delete(oldUid);
  store.set('reportDraftId.'+node,crypto.randomUUID());bindBugReportDraft();
  $('#bug-report-description').value='';
  showBugReportToast(data.report_id,data.worker);$('#bug-report-dialog').close();
  void loadTermList();
}

$('#bug-report-form').onsubmit = async event => {
  event.preventDefault();
  if (bugReportSending) return;
  const reportText = $('#bug-report-description').value;
  const description = reportText.trim();
  const error = $('#bug-report-error');
  if (!description) {
    error.textContent = '请先描述遇到的问题';
    $('#bug-report-description').focus();
    return;
  }
  const button = $('#bug-report-go');
  const attachments = [...bugReportDraftObject().attachments];
  const node = bugReportNode();
  const nodeError = bugReportNodeError(node);
  if (nodeError) {
    error.textContent = nodeError;
    return;
  }
  const origin = bugReportOrigin(node);
  const remote = HUB_MODE && !!origin.node_id && origin.node_id !== node;
  if (typeof staleBuildShown !== 'undefined' && staleBuildShown) {
    error.textContent = '页面已更新，请重新加载后再提交';
    return;
  }
  bugReportSending = true;
  button.disabled = true;
  setSendButtonBusy(button, '发送中');
  $('#bug-report-add').disabled = true;
  error.textContent = '';
  renderBugReportItems();
  const snapshot = browserStateSnapshot('bug-report');
  browserAuditEvent('bug_report.requested', {
    ...snapshot.data, attachments: attachments.length,
    worker_node: node, origin_node: origin.node_id || '', remote,
  }, snapshot.content);
  try {
    bugReportDraftObject().text = reportText;
    await hydrateComposerDraft(BUG_REPORT_DRAFT_UID);
    const pending=bugReportDraftObject();
    const priorPayload=JSON.stringify({description,source:bugReportSource(),
      attachments:attachments.map(a=>({upload_id:a.uploaded?.upload_id,number:a.number})),origin});
    if (pending.requestId && pending.requestText===priorPayload) {
      const previous=await priorComposerSubmission(BUG_REPORT_DRAFT_UID,pending.requestId,true);
      if (previous?.worker) {completeBugReportSubmission(previous,node);return;}
    }
    if (!await persistComposerDraft(BUG_REPORT_DRAFT_UID)) throw new Error(bugReportDraftObject().storageError || '报告草稿保存失败');
    // 与对话发送一致：同一批附件共用一个编号目录，失败的附件保留在卡片上重试。
    const uploaded = [];
    let attachmentId = null;
    for (let i = 0; i < attachments.length; i++) {
      setSendButtonBusy(button, `上传 ${i + 1}/${attachments.length}`);
      if (attachments[i].staging) await attachments[i].staging;
      const result = await uploadComposerAttachment(
        attachments[i], BUG_REPORT_DRAFT_UID, attachmentId, { node, render: renderBugReportItems });
      attachmentId ||= result.attachment_id;
      uploaded.push({upload_id:result.upload_id,number:attachments[i].number});
    }
    const terminalName = takenOver(S.sel) || (T.uid === S.sel ? T.name : '') || '';
    let captured = null;
    if (remote) {
      setSendButtonBusy(button, '抓取中');
      captured = await captureBugReportContext(origin.node_id, S.sel || '', terminalName);
    }
    setSendButtonBusy(button, '提交中');
    const source = bugReportSource();
    store.set('bugReportSource', source);
    const reportDraft=bugReportDraftObject();
    const requestText=JSON.stringify({description,source,attachments:uploaded,origin});
    if (reportDraft.requestText!==requestText || !reportDraft.requestId) {
      reportDraft.requestText=requestText;reportDraft.requestId=crypto.randomUUID();
    }
    if (!await persistComposerDraft(BUG_REPORT_DRAFT_UID)) throw new Error(reportDraft.storageError || '报告草稿保存失败');
    // 远端抓取时会话与终端引用不再随请求下发：中央站要求 uid 与 _node 指向
    // 同一台机器，问题会话的引用改由 origin.uid 与 captured 携带。
    const d = await post('api/bug-report', {
      ...(HUB_MODE ? {_node: node} : {}),
      draft_uid:BUG_REPORT_DRAFT_UID,draft_revision:reportDraft.revision,request_id:reportDraft.requestId,
      description, uid: remote ? '' : (S.sel || ''), page_id: TERM_PAGE_ID, source,
      terminal_name: remote ? '' : terminalName, snapshot, attachments: uploaded,
      origin, ...(captured ? {captured} : {}),
      cols: Math.max(80, T.term?.cols || 120), rows: Math.max(24, T.term?.rows || 36),
    });
    if (d.error) {
      error.textContent = d.error;
      return;
    }
    completeBugReportSubmission(d,node);
  } catch (failure) {
    error.textContent = `提交失败：${failure.message || failure}`;
  } finally {
    bugReportSending = false;
    button.disabled = false;
    setSendButtonBusy(button, '');
    $('#bug-report-add').disabled = false;
    renderBugReportItems();
  }
};

// ---------------------------------------------------------------- 新建会话
function suggestedSessionDir(cwd) {
  const path = String(cwd || '').replace(/\/+$/, '') || '/';
  // CLI/SDK 经常在这些易失根目录里生成一次性测试会话。它们仍属于会话
  // 历史，但不该因一次自动任务污染“最近使用”的新建目录建议。
  return path.startsWith('/') && !['/tmp', '/var/tmp', '/dev/shm'].some(
    root => path === root || path.startsWith(root + '/'));
}

function commonSessionDirs() {
  const dirs = new Map();
  for (const s of S.sessions) {
    if (HUB_MODE && s.node_id !== newNodeId()) continue;
    const cwd = String(s.cwd || '');
    if (!suggestedSessionDir(cwd)) continue;
    const row = dirs.get(cwd) || { cwd, count: 0, updated: '' };
    row.count++;
    if ((s.updated || '') > row.updated) row.updated = s.updated || '';
    dirs.set(cwd, row);
  }
  for (const [i, cwd] of store.get(newDirsKey(), []).entries()) {
    if (!cwd?.startsWith('/')) continue;
    const row = dirs.get(cwd) || { cwd, count: 0, updated: '' };
    row.recent = 20 - i;
    dirs.set(cwd, row);
  }
  const home = newNodeCapabilities().home;
  if (home && !dirs.has(home)) dirs.set(home, { cwd: home, count: 0, updated: '' });
  return [...dirs.values()].sort((a, b) =>
    (b.recent || 0) - (a.recent || 0) || b.count - a.count
    || b.updated.localeCompare(a.updated) || a.cwd.localeCompare(b.cwd));
}

const CWD_COMPLETION_DELAY = 120;
const cwdCompletion = {
  timer: null, abort: null, sequence: 0,
  rows: [], completions: [], forValue: '', active: -1, mode: 'common', common: [],
};

function canCompleteCwd(value) {
  const path = String(value || '').trim();
  return path.startsWith('/') || path === '~' || path.startsWith('~/');
}

function cancelCwdCompletionRequest() {
  if (cwdCompletion.timer) clearTimeout(cwdCompletion.timer);
  cwdCompletion.timer = null;
  cwdCompletion.abort?.abort();
  cwdCompletion.abort = null;
  cwdCompletion.sequence++;
}

function closeCwdPicker() {
  const input = $('#new-cwd'), picker = $('#new-cwd-picker');
  cancelCwdCompletionRequest();
  cwdCompletion.rows = [];
  cwdCompletion.completions = [];
  cwdCompletion.forValue = '';
  cwdCompletion.active = -1;
  $('#new-cwd-options').replaceChildren();
  picker.hidden = true;
  input.setAttribute('aria-expanded', 'false');
  input.removeAttribute('aria-activedescendant');
}

function cwdOption(path, meta = '', kind = 'recent') {
  return { path: String(path || ''), meta: String(meta || ''), kind };
}

function cwdPathKey(path) {
  const value = String(path || '');
  return value === '/' ? value : value.replace(/\/+$/, '');
}

function matchingRecentCwdOptions(value = '') {
  const query = String(value || '').trim().toLocaleLowerCase();
  return cwdCompletion.common
    .filter(row => !query || String(row.cwd || '').toLocaleLowerCase().includes(query))
    .map(row => cwdOption(row.cwd, row.count ? `${row.count} 个会话` : '', 'recent'));
}

function renderCwdOptions(value, recentRows, completionRows = [], completionNote = '') {
  const input = $('#new-cwd'), picker = $('#new-cwd-picker');
  const box = $('#new-cwd-options');
  const rawRecent = recentRows.map(row => typeof row === 'string'
    ? cwdOption(row, '', 'recent') : row);
  const rawCompletions = completionRows.map(row => typeof row === 'string'
    ? cwdOption(row, '', 'completion') : row);
  const completionFirst = String(value || '').startsWith('/');
  const seen = new Set();
  const unique = rows => rows.filter(row => {
    const key = cwdPathKey(row.path);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
  const completions = completionFirst ? unique(rawCompletions) : [];
  const recent = unique(rawRecent);
  if (!completionFirst) completions.push(...unique(rawCompletions));
  const options = completionFirst
    ? [...completions, ...recent] : [...recent, ...completions];
  cwdCompletion.mode = value ? 'matching' : 'common';
  cwdCompletion.rows = options.map(row => row.path);
  // recent 与建议中的同一路径只画一次，但它仍是文件系统补全候选；
  // Tab 计算公共前缀时不能因为视觉去重而把它漏掉。
  cwdCompletion.completions = rawCompletions.map(row => row.path);
  cwdCompletion.forValue = value;
  cwdCompletion.active = -1;
  box.replaceChildren();
  $('#new-cwd-options-title').textContent = value ? '匹配目录' : '最近使用';
  picker.hidden = false;

  const section = label => {
    const heading = document.createElement('div');
    heading.className = 'new-cwd-section';
    heading.setAttribute('role', 'presentation');
    heading.textContent = label;
    box.appendChild(heading);
  };
  const note = message => {
    const messageNode = document.createElement('div');
    messageNode.className = 'new-cwd-empty';
    messageNode.textContent = message;
    box.appendChild(messageNode);
  };
  const addOption = ({path, meta, kind}, index) => {
    const option = document.createElement('button');
    option.type = 'button';
    option.id = `new-cwd-option-${index}`;
    option.className = 'new-cwd-option';
    option.dataset.cwdKind = kind;
    option.dataset.cwdOption = String(index);
    option.setAttribute('role', 'option');
    option.setAttribute('aria-selected', 'false');
    option.title = path;
    const label = document.createElement('span');
    label.className = 'new-cwd-option-path';
    label.textContent = path;
    option.appendChild(label);
    if (meta) {
      const detail = document.createElement('span');
      detail.className = 'new-cwd-option-meta';
      detail.textContent = meta;
      option.appendChild(detail);
    }
    box.appendChild(option);
  };

  if (!value) {
    recent.forEach(addOption);
    if (!recent.length) note('还没有使用过的目录');
  } else {
    let offset = 0;
    const addGroup = (label, rows, empty = '') => {
      if (!rows.length && !empty) return;
      section(label);
      rows.forEach((row, index) => addOption(row, offset + index));
      offset += rows.length;
      if (!rows.length && empty) note(empty);
    };
    if (completionFirst) {
      addGroup('补全建议', completions, completionNote);
      addGroup('最近匹配', recent);
    } else {
      addGroup('最近匹配', recent);
      addGroup('补全建议', completions, completionNote);
    }
    if (!recent.length && !completions.length && !completionNote) note('没有匹配的目录');
  }
  input.setAttribute('aria-expanded', 'true');
  input.removeAttribute('aria-activedescendant');
  $('#new-cwd-completion-status').textContent = options.length
    ? (value ? `${recent.length} 个最近匹配，${completions.length} 个补全建议`
             : `${recent.length} 个最近目录`)
    : (completionNote || '没有匹配的目录');
}

function renderCommonCwdOptions() {
  renderCwdOptions('', matchingRecentCwdOptions());
}

function setCwdCompletionActive(step) {
  const rows = cwdCompletion.rows;
  if (!rows.length) return;
  const old = cwdCompletion.active;
  const next = old < 0
    ? (step > 0 ? 0 : rows.length - 1)
    : (old + step + rows.length) % rows.length;
  cwdCompletion.active = next;
  const options = [...$('#new-cwd-options').querySelectorAll('[data-cwd-option]')];
  options.forEach((option, index) => {
    const active = index === next;
    option.classList.toggle('active', active);
    option.setAttribute('aria-selected', String(active));
  });
  const option = options[next];
  $('#new-cwd').setAttribute('aria-activedescendant', option.id);
  option.scrollIntoView({ block: 'nearest' });
}

function setCwdValue(value, refresh = true) {
  const input = $('#new-cwd');
  input.value = value;
  $('#new-session-error').textContent = '';
  input.focus();
  input.setSelectionRange(value.length, value.length);
  if (refresh) scheduleCwdCompletions();
}

function longestCommonPrefix(values) {
  if (!values.length) return '';
  let prefix = values[0];
  for (const value of values.slice(1)) {
    let i = 0;
    while (i < prefix.length && i < value.length && prefix[i] === value[i]) i++;
    prefix = prefix.slice(0, i);
    if (!prefix) break;
  }
  return prefix;
}

function applyCwdTabCompletion() {
  const input = $('#new-cwd');
  if (!cwdCompletion.rows.length) return;
  if (cwdCompletion.active >= 0) {
    setCwdValue(cwdCompletion.rows[cwdCompletion.active]);
    return;
  }
  const rows = cwdCompletion.completions;
  if (!rows.length) return;
  if (rows.length === 1) {
    setCwdValue(rows[0]);
    return;
  }
  const value = input.value.trim();
  const prefix = longestCommonPrefix(rows);
  if (prefix.length > value.length) {
    setCwdValue(prefix, false);
    cwdCompletion.forValue = prefix;
    $('#new-cwd-completion-status').textContent =
      `已补全公共前缀，仍有 ${rows.length} 个补全建议`;
  }
}

async function loadCwdCompletions(complete = false) {
  cancelCwdCompletionRequest();
  const input = $('#new-cwd');
  const value = input.value.trim();
  const recent = matchingRecentCwdOptions(value);
  if (SessionDockCapabilities.config.backend === 'rust' && !SessionDockCapabilities.allows('terminal_complete_dir')) {
    renderCwdOptions(value, recent, [], '请填写已配置白名单中的现有工作目录；不会自动创建目录。');
    return;
  }
  if (!canCompleteCwd(value)) {
    renderCwdOptions(value, recent);
    return;
  }
  const controller = new AbortController();
  const sequence = ++cwdCompletion.sequence;
  cwdCompletion.abort = controller;
  try {
    const params = new URLSearchParams({ path: value });
    const response = await fetch(appUrl(`api/term/complete-dir?${params}`),
      { signal: controller.signal, cache: 'no-store' });
    const data = await response.json();
    if (sequence !== cwdCompletion.sequence || input.value.trim() !== value) return;
    const rows = response.ok && Array.isArray(data.directories)
      ? data.directories.filter(path => typeof path === 'string' && canCompleteCwd(path)).slice(0, 24)
      : [];
    renderCwdOptions(value, recent, rows, response.ok
      ? (rows.length ? '' : '没有补全建议')
      : (data.error || '目录补全暂不可用'));
    if (complete) applyCwdTabCompletion();
  } catch (error) {
    if (error.name !== 'AbortError' && sequence === cwdCompletion.sequence) {
      renderCwdOptions(value, recent, [], '目录补全暂不可用');
    }
  } finally {
    if (cwdCompletion.abort === controller) cwdCompletion.abort = null;
  }
}

function scheduleCwdCompletions() {
  cancelCwdCompletionRequest();
  const value = $('#new-cwd').value.trim();
  if (!value) {
    renderCommonCwdOptions();
    return;
  }
  const recent = matchingRecentCwdOptions(value);
  if (!canCompleteCwd(value)) {
    renderCwdOptions(value, recent);
    return;
  }
  renderCwdOptions(value, recent, [], '正在查找目录…');
  cwdCompletion.timer = setTimeout(() => loadCwdCompletions(false), CWD_COMPLETION_DELAY);
}

function openNewSessionDialog() {
  const dialog = $('#new-session-dialog');
  closeCwdPicker();
  newCreateAttempt = null;
  prepareNewNode();
  refreshNewNodeFields();
  dialog.showModal();
  renderCommonCwdOptions();
  setTimeout(() => { $('#new-cwd').focus(); $('#new-cwd').select(); }, 0);
}

function selectPendingSidebarRow(uid, added) {
  const row = added ? null : document.querySelector(`#side .item[data-uid="${CSS.escape(uid)}"]`);
  if (!row) {
    renderSide();
    return;
  }
  document.querySelectorAll('#side .item.sel').forEach(item => item.classList.remove('sel'));
  row.classList.add('sel');
}

function showNewSessionStage(info) {
  if (!T.pendingModes.has(info.name)) T.pendingModes.set(info.name, T.mode);
  // 临时会话也必须完整切换视图；列表或字体慢时不能继续显示/操作旧终端。
  inflight?.abort();
  closeWatch();
  closeTermPane(true);
  progressDone();
  // create 返回后 term/list 可能还没拉完；先把服务端刚确认的新 tmux 放进本地
  // pending，详情页的终端切换、输入框和附件可以立即使用。
  const added = !T.pending.some(x => x.name === info.name);
  if (added) T.pending.push({ ...info, started: pendingStartedAt(info) });
  cancelSearch(true);
  S.sel = pendingUid(info.name);
  rememberComposerSession(composerDraft(S.sel), info);
  S.agent = null;
  store.set('sel', S.sel);
  store.set('agent', null);
  // Clicking an existing pending row must reach term/claim immediately. A full
  // sidebar rebuild can take seconds for large histories and blocks attach.
  selectPendingSidebarRow(S.sel, added);
  showSessionCount(sidebarSessions().length);
  const src = SOURCES[info.source];
  const pendingTitle = info.title || `新建 ${src.name} 会话`;
  $('#detail').innerHTML = `<div class="dhead"><div class="dtitle">
    <button class="mobile-back" title="返回会话列表" aria-label="返回会话列表">←</button>
    <h2>${sessionIconMarkup(info.source, true, true)}<span>${esc(pendingTitle)}</span></h2>
    <div class="dhead-actions" aria-label="会话操作">
      <button class="iconbtn" id="a-term" title="切换到终端" aria-label="切换到终端">${uiIcon('terminal')}</button>
      ${sessionActionsMarkup(`
      <button class="session-menu-action" data-report-bug title="报告当前会话问题"
        aria-label="报告当前会话问题">${uiIcon('bug')}</button>
      <button class="session-menu-action danger" id="a-session-action"></button>
      `, `
    <div class="dmeta"><span id="mcount-total">0 条消息</span>
      ${info.node_name ? `<span class="meta-node node-badge" data-node-color="${nodeColor(info.node_name)}">${esc(info.node_name)}</span>` : ''}
      <span class="meta-secondary"><code>${esc(shortCwd(info.cwd, 999))}</code></span>
      <span class="meta-source">${esc(src.name)}</span></div>`)}
    </div></div>
  </div><div class="empty new-session-wait">${SessionDockCapabilities.config.backend === 'rust'
    ? esc(pendingStageMessage(info)) : ''}</div>`;
  $('#detail .mobile-back').onclick = showMobileList;
  bindConsoleButton($('#a-term'), S.sel);
  renderPendingSessionAction(info);
  bindSessionActions($('#detail .dhead'));
  if (typeof auditDetailRendered === 'function') auditDetailRendered('new-session', {name: info.name});
  showMobileDetail();
  T.uid = S.sel;
  renderComposer();
  renderTakeoverBtn();
}

// Bug-report worker rows (`kind: "bug-report"`) carry the manifest status so
// an injection failure is visible instead of a silently idle CLI.
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
    default: return s.source === 'shell' ? '交互式终端' : '等待首条消息';
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
  if (info.source !== 'shell' && !info.declared_sid && pendingRecordMissing(info))
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

function renderPendingSessionAction(info, button = $('#a-session-action')) {
  if (!button || S.sel !== pendingUid(info.name)) return;
  // 行已不在列表里（别的页面删了、或已归档）：按已结束处理，绝不按旧 receipt 显示"停止"。
  const current = pendingSessionRow(info.name) || { ...info, running: false, stale: true, state: 'exited' };
  const stop = pendingShellRunning(current);
  const label = stop ? '停止会话' : '删除会话';
  button.innerHTML = uiIcon(stop ? 'power' : 'trash');
  button.title = button.ariaLabel = label;
  if (typeof labelSessionAction === 'function') labelSessionAction(button);
  button.onclick = () => stop ? stopPendingSession(current, button) : deletePendingSession(current, button);
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
    if (!confirm(`停止会话「${pendingTitle(info)}」?\n\n停止后才可以删除会话记录。`)) return;
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
    } catch (error) { alert(error.message || '停止失败，请重试。'); }
    finally { if (button) button.disabled = false; }
    return;
  }
  return deleteSessions([pendingUid(info.name)], button);
}

async function deletePendingSession(info, button) {
  if (info.source === 'shell' && SessionDockCapabilities.config.backend === 'rust'
      && !confirm(`删除会话「${pendingTitle(info)}」?\n\n会话记录和它的录制会一并删除，无法恢复。`)) return;
  if (button) button.disabled = true;
  try {
    await discardPendingSession(info);
    await loadTermList();
    if (typeof loadSessions === 'function') await loadSessions(true);
  } catch (error) { alert(error.message || '删除失败，请重试。'); }
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
  if (pending.source === 'shell' && (running || retained || pending.recording))
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

  const draft = composerDrafts.get(uid);
  for (const attachment of draft?.attachments || []) {
    if (attachment.preview) URL.revokeObjectURL(attachment.preview);
  }
  composerDrafts.delete(uid);
  deleteComposerDraftStorage(uid);
  if (composerUid === uid) composerUid = null;
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
      const reopen = (!$('#termpane').classList.contains('hidden') && T.name === current.name)
        || T.openViews.has(current.name);
      const existing = T.views.get(current.name);
      if (existing && existing.bindingUid === pendingId) {
        rememberTermOpen(current.name, false);
        disposeTermView(current.name);
      }
      T.uid = linked.uid;
      await openSession(linked.uid);
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
        const draft = composerDrafts.get(pendingId);
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

let newCreateAttempt = null;
function newSessionRequestId(source, cwd) {
  const key = JSON.stringify([newNodeId(), source, cwd]);
  if (newCreateAttempt?.key !== key) newCreateAttempt = {key,
    rows: termRows(),
    id: globalThis.crypto?.randomUUID?.() || `${Date.now()}-${Math.random().toString(36).slice(2)}`};
  return newCreateAttempt.id;
}

async function createNewSession(e) {
  e.preventDefault();
  const source = $('#new-session-dialog input[name="new-source"]:checked')?.value;
  const cwd = $('#new-cwd').value.trim();
  const go = $('#new-session-go'), error = $('#new-session-error');
  error.textContent = '';
  if (!source) { error.textContent = '没有可用的会话类型'; return; }
  if (!cwd) { error.textContent = '请选择启动目录'; return; }
  go.disabled = true;
  go.textContent = '创建中…';
  const dirsKey = newDirsKey();
  try {
    const requestId = newSessionRequestId(source, cwd);
    const request = { source, cwd, cols: 120, rows: newCreateAttempt.rows,
      request_id: requestId, ...(HUB_MODE ? {_node: newNodeId()} : {}) };
    let d = await post('api/term/create', request);
    if (d.needs_create) {
      const target = String(d.cwd || cwd);
      if (!confirm(`启动目录不存在：\n${target}\n\n是否创建该目录并继续？`)) {
        $('#new-cwd').focus();
        return;
      }
      go.textContent = '创建目录中…';
      d = await post('api/term/create', { ...request, cwd: target, create_cwd: true });
    }
    if (d.error) { error.textContent = d.error; return; }
    const recent = [d.cwd, ...store.get(dirsKey, []).filter(x => x !== d.cwd)].slice(0, 8);
    store.set(dirsKey, recent);
    $('#new-session-dialog').close();
    await openPendingSession(d);
    // The backend instance is ready; list refresh must not block the conversation.
    void loadTermList();
  } catch (err) {
    error.textContent = err.message || '创建失败';
  } finally {
    go.disabled = false;
    go.textContent = '创建';
  }
}

$('#new-session').onclick = openNewSessionDialog;
$('#new-session-form').onsubmit = createNewSession;
$('#new-session-dialog .modal-close').onclick = () => $('#new-session-dialog').close();
$('#new-session-dialog .modal-cancel').onclick = () => $('#new-session-dialog').close();
$('#new-cwd').oninput = () => {
  $('#new-session-error').textContent = '';
  scheduleCwdCompletions();
};
$('#new-cwd').onkeydown = e => {
  if (e.isComposing) return;
  if (e.key === 'Tab' && !e.shiftKey && canCompleteCwd(e.currentTarget.value)) {
    e.preventDefault();
    if (cwdCompletion.mode === 'matching' && cwdCompletion.completions.length
        && cwdCompletion.forValue === e.currentTarget.value.trim()) {
      applyCwdTabCompletion();
    } else {
      loadCwdCompletions(true);
    }
  } else if (e.key === 'ArrowDown' && cwdCompletion.rows.length) {
    e.preventDefault();
    setCwdCompletionActive(1);
  } else if (e.key === 'ArrowUp' && cwdCompletion.rows.length) {
    e.preventDefault();
    setCwdCompletionActive(-1);
  } else if (e.key === 'Enter' && cwdCompletion.active >= 0) {
    e.preventDefault();
    setCwdValue(cwdCompletion.rows[cwdCompletion.active]);
  }
};
$('#new-cwd-options').onclick = e => {
  const option = e.target.closest('[data-cwd-option]');
  if (!option) return;
  e.preventDefault();
  const path = cwdCompletion.rows[Number(option.dataset.cwdOption)];
  if (path) setCwdValue(path);
};
$('#new-session-dialog').addEventListener('close', closeCwdPicker);
$('#new-session-dialog').addEventListener('click', e => {
  if (e.target === $('#new-session-dialog')) $('#new-session-dialog').close();
});

const termRows = () => Math.max(10, Math.floor(T.height / (termFontSize() * 1.31)));

/** 详情页头部那个按钮的文案随状态变。 */
function renderTakeoverBtn() {
  const b = $('#a-term');
  if (!b) return;
  const name = takenOver(S.sel);
  const replacement = name ? null
    : linkedTermSession(S.sel, { followReplacement: true });
  const paneOpen = !!name && !$('#termpane').classList.contains('hidden');
  const ptyOnly = typeof sessionIsPtyOnly === 'function' && sessionIsPtyOnly(S.sel);
  const switchToChat = paneOpen && (ptyOnly ? T.mode === 'full' : (MOBILE.matches || T.mode === 'full'));
  const terminalVisible = paneOpen && (ptyOnly || MOBILE.matches || T.mode !== 'collapsed');
  const label = replacement ? '切换到当前会话终端'
    : !name ? '接管会话'
    : ptyOnly ? (switchToChat ? '切换到对话' : '切换到终端')
    : MOBILE.matches ? (paneOpen ? '切换到对话' : '切换到终端')
    : switchToChat ? '切换到对话' : '切换到终端';
  b.innerHTML = uiIcon(switchToChat ? 'chat' : 'terminal');
  b.title = b.ariaLabel = label;
  b.setAttribute('aria-expanded', String(terminalVisible));
  b.classList.toggle('on', !!name);
  paintConsoleAvailability(b, S.sel, S.agent);
  renderComposer();
  if (typeof auditConsoleButton === 'function') auditConsoleButton('takeover-btn');
}

/** 终端面板每次开合/换模式都记一笔 terminal.pane：谁触发的、之前之后什么状态。 */
function auditTermPane(action, extra = {}) {
  if (typeof browserAuditEvent !== 'function') return;
  const pane = $('#termpane');
  browserAuditEvent('terminal.pane', {
    action, name: T.name, uid: T.uid, mode: T.mode, mobile: MOBILE.matches,
    visible: !!pane && !pane.classList.contains('hidden'),
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
  return !String(uid || '').startsWith('tmux:');
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
  if (consoleRendererFor(row) !== 'grid' || typeof globalThis.GridTerm !== 'function') return false;
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
  }) : new Terminal({
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
    : new FitAddon.FitAddon();
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
  if (!grid && globalThis.Unicode11Addon?.Unicode11Addon) {
    try {
      view.unicode11 = new Unicode11Addon.Unicode11Addon();
      term.loadAddon(view.unicode11);
      term.unicode.activeVersion = '11';
    } catch { view.unicode11 = null; }
  }
  term.open(host);
  // Edge 在任何聚焦的可编辑元素插入点旁挂一个 Copilot“撰写”浮动按钮（一个蓝点），
  // 它会贴着 xterm 这个隐藏的 IME textarea 跟随光标。Edge 124+ 认这个属性，
  // 同时关掉文本预测；其它浏览器忽略。
  term.textarea?.setAttribute('writingsuggestions', 'false');
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
  if (!grid && shouldUseTermWebgl() && globalThis.WebglAddon?.WebglAddon) {
    try {
      const webgl = new WebglAddon.WebglAddon();
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
    d = applyTermCtrl(d);
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
    if (/[\r\n]/.test(d)) {
      notePendingInput(name);
      if (claudeRewinds.has(name)) scheduleClaudeRewindSync(name);
    }
  });
  // 专用 server 不让 tmux 接管滚动：外层不进 alternate screen，直接使用
  // xterm 的正常 scrollback。改造前遗留在默认 server 的会话仍走旧兼容路径。
  term.attachCustomWheelEventHandler(e => {
    if (T.name !== name) return true;
    // 录制回放和已退出的画面没有宿主 copy-mode；滚轮留给 xterm / 面板滚动条。
    if (view.replay || view.ended || view.revoked) return true;
    if (T.list?.find(x => x.name === name)?.server === 'sessiondock') return true;
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
  view.term.write(chunk, () => scheduleCodexSideThreadScan(view));
}

function dropTermSyncHold(view) {
  if (view.syncHoldTimer) clearTimeout(view.syncHoldTimer);
  view.syncHoldTimer = null;
  view.syncHold = null;
}

function termPaneRenderable(view = currentTermViewObject()) {
  if (!view || view !== currentTermViewObject()) return false;
  const pane = $('#termpane');
  if (pane.classList.contains('hidden')) return false;
  if (!MOBILE.matches && T.mode === 'collapsed' && !(typeof sessionIsPtyOnly === 'function' && sessionIsPtyOnly()))
    return false;
  if (pane.classList.contains('term-collapsed')) return false;
  // 手机从桌面布局切回会话列表时，#right 会由祖先的 display:none 隐藏，
  // 但 #termpane 本身没有 hidden 类。FitAddon 在这种容器上会返回内部最小值
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
  // 软键盘会把 FitAddon 量到的行数砍掉一截。把这个尺寸发给 PTY 会让 CLI
  // 重排并丢掉编辑区，会话模式 CHECK/SEND 随即 409 cli_not_ready。网页已经
  // 按 visual viewport 让位，这里保持键盘收起时的行列，只把画面钉在提示符。
  if (typeof visualKeyboardOpen === 'function' && visualKeyboardOpen()) {
    try { view.term.scrollToBottom(); } catch { /* disposed */ }
    return;
  }
  let dimensions;
  try { dimensions = view.fit.proposeDimensions(); } catch { return; }
  if (!dimensions || !Number.isFinite(dimensions.cols) || !Number.isFinite(dimensions.rows)) return;
  // FitAddon.fit() 会先调用私有 _renderService.clear()，DOM renderer 因而在每次
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
  // Terminal.resize 不会触发 renderer。重新激活时必须显式画回整个 viewport。
  if (resized || forceSync) repaintTermView(view);
}

function fitTerm(immediate = false, forceSync = false) {
  const view = currentTermViewObject();
  if (!termPaneRenderable(view)) return;
  if (view.fitFrame) cancelAnimationFrame(view.fitFrame);
  view.fitFrame = null;
  if (immediate) {
    performTermFit(view, forceSync);
    return;
  }
  // 浏览器最大化、拖边界会连续发 resize；每个动画帧最多 fit 一次。
  // 软键盘只改网页可视高度，performTermFit 会拒绝随之 SIGWINCH。
  view.fitFrame = requestAnimationFrame(() => {
    view.fitFrame = null;
    performTermFit(view, forceSync);
  });
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
  if (waitingPrompt && $('#termpane').classList.contains('hidden')) {
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
  if (!$('#termpane').classList.contains('hidden') && T.name === name) {
    renderTakeoverBtn();
    return;
  }
  auditTermPane('restore', {target: name});
  openTermPane(name, false, null, true);
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
  pane.classList.remove('hidden');
  layoutTermPane();
  renderTakeoverBtn();
  try { await terminalFontReady; } catch { /* 字体失败时继续用 Consola/monospace */ }
  if (openEpoch !== termOpenEpoch || pane.classList.contains('hidden')) return false;
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
  if (pane.classList.contains('hidden')) {
    return openTermPane(name, true, MOBILE.matches ? null : 'full');
  }
  if (typeof sessionIsPtyOnly === 'function' && sessionIsPtyOnly()) {
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
  if (!pane || pane.classList.contains('hidden')) return false;
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
  pane.classList.add('hidden');
  pane.classList.remove('term-collapsed');
  $('#right').classList.remove('term-full');
  renderTakeoverBtn();
}

function layoutTermPane() {
  const pane = $('#termpane');
  const right = $('#right');
  const desktop = !MOBILE.matches;
  const paneOpen = !pane.classList.contains('hidden');
  const shell = typeof sessionIsPtyOnly === 'function' && sessionIsPtyOnly();
  right.classList.toggle('shell-session', shell);
  right.classList.toggle('term-full', paneOpen && T.mode === 'full' && (desktop || shell));
  if (shell && paneOpen && T.mode !== 'full') {
    pane.classList.remove('term-collapsed');
    pane.style.removeProperty('--mobile-terminal-top');
    const detailHeight = $('#detail')?.offsetHeight || 0;
    const composerHeight = $('#composer')?.offsetHeight || 0;
    pane.style.height = Math.max(0, right.clientHeight - detailHeight - composerHeight) + 'px';
    return;
  }
  pane.classList.toggle('term-collapsed', desktop && paneOpen && T.mode === 'collapsed');
  if (MOBILE.matches) {
    pane.style.removeProperty('height');
    const rightTop = right.getBoundingClientRect().top;
    const headBottom = $('#detail > .dhead')?.getBoundingClientRect().bottom ?? rightTop;
    pane.style.setProperty('--mobile-terminal-top', `${Math.max(0, Math.round(headBottom - rightTop))}px`);
  } else {
    pane.style.removeProperty('--mobile-terminal-top');
    if (T.mode === 'collapsed') pane.style.height = '0px';
    else if (T.mode === 'full') {
      pane.style.height = Math.max(0, right.clientHeight - $('#detail').offsetHeight) + 'px';
    }
    else {
      const detailHeadHeight = $('#detail > .dhead')?.offsetHeight || 0;
      const composerHeight = $('#composer')?.offsetHeight || 0;
      const maxHeight = Math.max(0, right.clientHeight - detailHeadHeight - composerHeight);
      pane.style.height = Math.min(T.height, maxHeight) + 'px';
    }
  }
}

// `auto`：由布局恢复（选中会话、刷新、题卡）自动打开的 pty。进入对话页不该被
// 抢占问题打断：别处持有时静默放弃、留在对话页；只有用户主动打开 pty 才问。
async function claimTermOwnership(name, uid = T.uid, binding = {}, auto = false, direct = false) {
  const claim = async force => {
    try {
      return await post('api/term/claim', {name, page: TERM_PAGE_ID,
        ...(force ? {force: true} : {}), ...binding}, {timeoutMs: TERM_CLAIM_TIMEOUT_MS});
    } catch (error) {
      if (error.name !== 'TimeoutError') throw error;
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
    if (!confirm(`该终端正由${holder}控制。\n\n是否抢占终端？`)) return null;
    result = await claim(true);
  }
  if (result.error || !result.token) {
    ConsoleUI.errors.set(uid, result.error || '无法取得终端控制权');
    renderTakeoverBtn();
    if (!auto && !result.timeout) alert('打开终端失败：' + (result.error || '无法取得终端控制权'));
    return null;
  }
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
  alert(`终端已被${describeTermTaker(by, ip)}抢占，本页面的终端已关闭。`);
}

function renderTermOutputNotice(view) {
  const notice = $('#term-output-notice');
  if (!notice) return;
  const text = view?.outputNotice || '';
  notice.textContent = text;
  notice.hidden = !text;
  $('#termpane')?.classList.toggle('output-incomplete', !!text);
}

function recordHostExit(view, uid, event) {
  if (SessionDockCapabilities.config.backend !== 'rust') return false;
  const incomplete = event.code === 1011 && event.reason.startsWith('host output incomplete');
  if (!incomplete && !(event.code === 1000 && event.reason === 'host exited')) return false;
  const reason = incomplete
    ? `终端输出不完整：${event.reason}。已保留收到的尾部输出，不会自动重新连接。`
    : '终端进程已退出，已保留收到的输出。';
  const pendingRow = (T.pending || []).find(item => item.name === view.name);
  const shell = pendingRow?.source === 'shell'
    || (typeof sessionIsPtyOnly === 'function' && sessionIsPtyOnly(uid));
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
    view.keepOutput = true;
    if (T.name === view.name && !shell)
      closeTermPane(true);
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
  const pane = $('#termpane');
  const bar = $('#term-timeline');
  if (!pane || !bar) return;
  const tl = view?.replay ? view.timeline : null;
  pane.classList.toggle('replay', !!tl);
  if (!tl) return;
  const span = Math.max(0, tl.end - tl.start);
  const seek = $('#tl-seek');
  if (!tl.scrubbing) seek.value = span ? String(Math.round((tl.clock - tl.start) / span * 1000)) : '1000';
  const at = tl.scrubbing ? tl.start + Number(seek.value) / 1000 * span : tl.clock;
  $('#tl-time').textContent = `${replayTimeElapsed(at - tl.start)} / ${replayTimeElapsed(span)}`;
  seek.setAttribute('aria-valuetext', replayTimeElapsed(at - tl.start));
  seek.title = new Date(at).toLocaleTimeString();
  $('#tl-status').textContent = tl.live ? '只读回放' : '会话已结束 · 只读回放';
  const ticks = $('#tl-ticks');
  const tickKey = `${tl.start}:${tl.end}`;
  if (ticks.dataset.bounds !== tickKey) {
    ticks.dataset.bounds = tickKey;
    ticks.replaceChildren(...[0, .25, .5, .75, 1].map(fraction => {
      const tick = document.createElement('span');
      tick.textContent = replayTimeElapsed(span * fraction);
      return tick;
    }));
  }
  const play = $('#tl-play');
  play.textContent = tl.playing ? '❚❚' : '▶';
  play.title = play.ariaLabel = tl.playing ? '暂停' : (tl.atEnd ? '从头播放' : '播放');
  $('#tl-speed').value = String(tl.speed);
  $('#tl-live').hidden = !tl.live;
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

function bindTimeline() {
  const seek = $('#tl-seek');
  if (!seek) return;
  const view = () => { const v = currentTermViewObject(); return v?.replay && v.timeline ? v : null; };
  const target = v => v.timeline.start + Number(seek.value) / 1000 * Math.max(0, v.timeline.end - v.timeline.start);
  seek.addEventListener('pointerdown', () => { const v = view(); if (v) v.timeline.scrubbing = true; });
  seek.addEventListener('input', () => {
    const v = view();
    if (!v) return;
    v.timeline.scrubbing = true;
    renderTimeline(v);
  });
  seek.addEventListener('change', () => {
    const v = view();
    if (!v) return;
    v.timeline.scrubbing = false;
    timelineSeekTo(v, target(v));
    renderTimeline(v);
  });
  // A drag previews locally and submits once on release; intermediate seek
  // responses cannot pull the thumb back while the user is choosing a time.
  seek.addEventListener('pointerup', () => setTimeout(() => {
    const v = view();
    if (v?.timeline.scrubbing) {
      v.timeline.scrubbing = false;
      renderTimeline(v);
    }
  }, 0));
  seek.addEventListener('pointercancel', () => {
    const v = view();
    if (v) { v.timeline.scrubbing = false; renderTimeline(v); }
  });
  $('#tl-play').addEventListener('click', () => {
    const v = view();
    if (!v) return;
    const tl = v.timeline;
    if (tl.playing) { tl.playing = false; timelineSend(v, { t: 'pause' }); }
    else { tl.playing = true; tl.atEnd = false; timelineSend(v, { t: 'play', speed: tl.speed }); }
    renderTimeline(v);
  });
  $('#tl-speed').addEventListener('change', e => {
    const v = view();
    if (!v) return;
    v.timeline.speed = Number(e.target.value) || 1;
    if (v.timeline.playing) timelineSend(v, { t: 'play', speed: v.timeline.speed });
    renderTimeline(v);
  });
  $('#tl-live').addEventListener('click', () => {
    const v = view();
    if (!v) return;
    v.timeline.playing = false;
    timelineSend(v, { t: 'live' });
    renderTimeline(v);
  });
}
bindTimeline();

function attachRecordingReplay(view, row, uid) {
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

function attachTerm(name, auto = false, directClaim = false) {
  const view = ensureTerm(name);
  if (view.attachPromise) return view.attachPromise;
  const job = attachOwnedTerm(view, true, auto, directClaim).finally(() => {
    if (view.attachPromise === job) view.attachPromise = null;
  });
  view.attachPromise = job;
  return job;
}

async function attachOwnedTerm(view, allowRefresh = true, auto = false, directClaim = false) {
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
  const active = !$('#termpane').classList.contains('hidden') && T.name === name;
  if (active) activateTermView(view);
  cancelTermReconnect(view);
  dropTermSocket(view);
  const token = await claimTermOwnership(name, uid, binding, auto, directClaim);
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
      view.keepOutput = true;
      if (T.name === name && !(typeof sessionIsPtyOnly === 'function' && sessionIsPtyOnly(uid)))
        closeTermPane(true);
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
  const ws = view.ws;
  view.ws = null;                        // 先失效引用，close 回调便不会误判成意外断线
  if (T.name === view.name) T.ws = null;
  if (ws) { try { ws.close(); } catch {} }
}

/** 网络短断后自动恢复。tmux 才是会话本体，WebSocket 只是可随时重建的视图。 */
function scheduleTermReconnect(view = currentTermViewObject()) {
  if (!view || view.revoked || document.hidden || !navigator.onLine || view.reconnectTimer) return;
  const stillAlive = [...(T.list || []), ...(T.pending || [])].some(x => x.name === view.name);
  if (!stillAlive) return;
  const delay = view.reconnectDelay;
  view.reconnectTimer = setTimeout(() => {
    view.reconnectTimer = null;
    if (!T.views.has(view.name) || document.hidden) return;
    view.reconnectDelay = Math.min(8000, Math.round(view.reconnectDelay * 1.8));
    attachTerm(view.name);
  }, delay);
}

/** 手机锁屏会冻结一个看似仍 OPEN、实际已经失效的 socket；恢复时必须强制换新。 */
function reconnectTerm(view = currentTermViewObject()) {
  if (!view || document.hidden || !navigator.onLine) return;
  if (view === currentTermViewObject() && !termPaneRenderable(view)) return;
  attachTerm(view.name);
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
  renderTimeline(null);
  renderTermOutputNotice(null);
}

function disposeTermView(name) {
  const view = T.views.get(name);
  if (!view) return;
  const active = T.name === name;
  setCodexSideThreadState(view, false);
  cancelTermReconnect(view);
  dropTermSocket(view);
  try { view.term.dispose(); } catch { /* 已被浏览器清理 */ }
  view.host.remove();
  T.views.delete(name);
  if (active) {
    deactivateTermView();
    $('#termpane').classList.add('hidden');
    $('#termpane').classList.remove('term-collapsed');
    $('#right').classList.remove('term-full');
  }
}

// ---------------------------------------------------------------- 输入框
// 已接管的会话在消息流底部给个输入框, 不必展开整个终端就能说话。
const COMPOSER_MAX_FILES = 12;
const COMPOSER_MAX_FILE_BYTES = 512 * 1024 * 1024;
const ATTACH_ACCEPT = { image: 'image/*', video: 'video/*', audio: 'audio/*', file: '' };
const composerDrafts = new Map();
const composerDraftAliases = new Map();
const composerInputHistoryCache = new Map();
const composerHistoryPicker = {
  open: false, uid: null, items: [], index: -1, seq: 0,
};
let composerUid = null;
let composerDraftSeq = 0;
let lastMessageSelection = '';
let lastMessageSelectionUid = null;

// Input content is owned by the server. Only unsent File objects live in RAM.
const conversationSendEnabled = () => SessionDockCapabilities.config.conversation_send === true;
const newComposerDraft = () => ({text:'', attachments:[], quotes:[], nextAttachmentNumber:1,
  revision:0, editVersion:0, savedVersion:0});
const snapshotComposerAttachments = items => items.map(item => ({...item,
  uploaded:item.uploaded ? {...item.uploaded} : null}));
const composerDraftFiles = draft => draft.attachments;
const composerHydrations = new Map();
let composerDraftWrites = Promise.resolve();
const composerSaveQueues = new Map();
function composerDraftOwner(uid) {
  const seen = new Set();
  while (composerDraftAliases.has(uid) && !seen.has(uid)) {
    seen.add(uid); uid = composerDraftAliases.get(uid);
  }
  return uid;
}
function restoreComposerDraftRecord(record) {
  const draft = Object.assign(newComposerDraft(), record || {});
  delete draft.saved;
  // Optional/nullable fields in old server records must retain editor defaults.
  draft.text ??= '';
  draft.attachments ??= [];
  draft.quotes ??= [];
  for (const attachment of draft.attachments) {
    attachment.status = ''; attachment.preview = '';
  }
  return ensureComposerAttachmentNumbers(draft);
}
function composerDraft(uid = composerUid, create = true) {
  uid = composerDraftOwner(uid);
  if (!uid) return null;
  if (!composerDrafts.has(uid) && create) composerDrafts.set(uid, newComposerDraft());
  if (create) hydrateComposerDraft(uid);
  return composerDrafts.get(uid) || null;
}
function composerDraftRecord(draft, uid) {
  return {text:draft.text, quotes:draft.quotes.map(item => ({...item})),
    attachments:draft.attachments.map(item => ({id:item.id, number:item.number, kind:item.kind,
      uploaded:item.uploaded, file:{name:item.file.name, size:item.file.size,
        type:item.file.type, lastModified:item.file.lastModified}})),
    nextAttachmentNumber:draft.nextAttachmentNumber, requestId:draft.requestId,
    requestText:draft.requestText, session:{...draft.session, uid},
    ...(draft.report_prompt ? {report_prompt:draft.report_prompt,report_text:draft.report_text} : {})};
}
async function priorComposerSubmission(uid,id,report=false) {
  const query=new URLSearchParams({uid,[report?'report_request_id':'request_id']:id});
  const response=await fetch(appUrl('api/session/conversation?'+query),{cache:'no-store'});
  const data=await response.json();
  if (response.status===404) return null;
  if (!response.ok || data.error) throw new Error(data.error || `HTTP ${response.status}`);
  return data;
}
function acceptComposerServerRevision(draft,row) {
  if (!row || row.revision<=draft.revision) return;
  const value=row.value;
  const empty=!value?.text && !value?.attachments?.length && !value?.quotes?.length;
  const same=value?.text===draft.text
    && JSON.stringify((value?.attachments || []).map(a=>a.id))===JSON.stringify(draft.attachments.map(a=>a.id))
    && JSON.stringify(value?.quotes || [])===JSON.stringify(draft.quotes);
  if (empty || same) draft.revision=row.revision;
}
async function consumeComposerSubmission(uid,text,attachments,quotes) {
  const owner=composerDraftOwner(uid),draft=composerDrafts.get(owner);
  if (!draft) return;
  delete draft.requestId;delete draft.requestText;delete draft.report_prompt;delete draft.report_text;
  if (draft.text===text) draft.text='';
  const files=new Set(attachments.map(a=>a.id)),quoted=new Map(quotes.map(q=>[q.id,q.text]));
  for (const item of draft.attachments.filter(a=>files.has(a.id))) {
    if (item.preview) URL.revokeObjectURL(item.preview);
  }
  draft.attachments=draft.attachments.filter(a=>!files.has(a.id));
  draft.quotes=draft.quotes.filter(q=>quoted.get(q.id)!==q.text);
  if (!draft.text && !draft.attachments.length && !draft.quotes.length) draft.nextAttachmentNumber=1;
  await persistComposerDraft(owner);refreshComposerDraft(owner);
}
async function readServerComposerDraft(uid) {
  const response = await fetch(appUrl('api/session/conversation?' + new URLSearchParams({uid})),
    {cache:'no-store'});
  const data = await response.json();
  if (!response.ok || data.error) throw new Error(data.error || `HTTP ${response.status}`);
  return data.draft;
}
// Read old browser copies once; never write new input into browser storage.
// Migration copies legacy File bytes to the server before removing verified originals.
let legacyComposerDatabase;
async function oldComposerDatabase() {
  if (legacyComposerDatabase !== undefined) return legacyComposerDatabase;
  legacyComposerDatabase = null;
  if (!globalThis.indexedDB?.databases) return null;
  const databases = await indexedDB.databases();
  if (!databases.some(db => db.name === STORAGE_PREFIX + 'composer-drafts')) return null;
  legacyComposerDatabase = await new Promise((resolve, reject) => {
    const request = indexedDB.open(STORAGE_PREFIX + 'composer-drafts');
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
  return legacyComposerDatabase;
}
async function importLegacyComposer(uid) {
  let record = store.get('composerDraft.' + uid, null);
  const db = await oldComposerDatabase();
  if (db) {
    const indexed = await new Promise((resolve, reject) => {
      const request = db.transaction('drafts').objectStore('drafts').get(uid);
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    if (indexed && (!record || (+indexed.revision || 0) > (+record.revision || 0))) record = indexed;
  }
  const oldQueue=store.get('queuedMessages',[]).find(([key])=>key===uid)?.[1];
  if (oldQueue?.length) {
    const imported=await post('api/session/conversation/import',{uid,value:{legacy_queue:oldQueue}});
    if (imported.error) throw new Error(imported.error);
    const remaining=store.get('queuedMessages',[]).filter(([key])=>key!==uid);
    if (remaining.length) store.set('queuedMessages',remaining);
    else localStorage.removeItem(STORAGE_PREFIX+'queuedMessages');
  }
  if (!record || record.removed) return null;
  const converted=structuredClone(record);
  const attachments=[...(converted.attachments || []),
    ...(converted.saved || []).flatMap(item=>item.attachments || [])];
  if (db) for (const item of attachments) {
    if (item.uploaded?.upload_id) continue;
    const file=await new Promise((resolve,reject) => {
      const request=db.transaction('files').objectStore('files').get(uid+'\0'+item.id);
      request.onsuccess=()=>resolve(request.result);request.onerror=()=>reject(request.error);
    });
    if (!(file instanceof Blob)) continue;
    const url=new URL(appUrl('api/session/conversation/attachment'));
    url.searchParams.set('uid',uid);url.searchParams.set('id','legacy-'+item.id);
    url.searchParams.set('name',item.file.name);
    const response=await fetch(url,{method:'POST',headers:{'Content-Type':item.file.type || 'application/octet-stream'},body:file});
    const uploaded=await response.json();
    if (!response.ok || uploaded.error) throw new Error(uploaded.error || `HTTP ${response.status}`);
    item.uploaded={...uploaded,uid};
  }
  const result=await post('api/session/conversation/import',{uid,value:converted});
  if (result.error) throw new Error(result.error);
  // Delete browser originals only after every existing File has a durable
  // server reference. Unknown/missing bytes keep their original recovery store.
  if (attachments.every(item=>item.uploaded?.upload_id)) {
    if (db) await new Promise((resolve,reject) => {
      const transaction=db.transaction(['drafts','files'],'readwrite');
      transaction.objectStore('drafts').delete(uid);
      const prefix=uid+'\0';
      const request=transaction.objectStore('files').openKeyCursor(IDBKeyRange.bound(prefix,prefix+'\uffff'));
      request.onsuccess=()=>{const cursor=request.result;if (cursor){transaction.objectStore('files').delete(cursor.key);cursor.continue();}};
      transaction.oncomplete=resolve;transaction.onabort=transaction.onerror=()=>reject(transaction.error);
    });
    localStorage.removeItem(STORAGE_PREFIX+'composerDraft.'+uid);
    const remaining=store.get('composerDraftUids',[]).filter(key=>key!==uid);
    if (remaining.length) store.set('composerDraftUids',remaining);
    else localStorage.removeItem(STORAGE_PREFIX+'composerDraftUids');
  }
  return {record:converted,db};
}
function hydrateComposerDraft(uid) {
  uid = composerDraftOwner(uid);
  if (composerHydrations.has(uid)) return composerHydrations.get(uid);
  if (!conversationSendEnabled()) return Promise.resolve();
  const task = (async () => {
    const draft = composerDrafts.get(uid);
    if (!draft) return;
    draft.loading = true;
    try {
      const legacy = await importLegacyComposer(uid);
      const row = await readServerComposerDraft(uid);
      // A page that has never saved adopts the server revision even when
      // typing began before this read returned; keeping revision 0 would get
      // every later save refused. A page that has saved keeps its CAS
      // baseline, and the save path rebases on a conflict.
      if (!draft.savedVersion) draft.revision = row.revision;
      if (uid.startsWith('report:') && row.value?.session?.kind==='bug-report'
          && row.value.session.uid && !row.value.session.uid.startsWith('report:')) {
        draft.handedOffSession=row.value.session;return;
      }
      if (!draft.editVersion && row.value && !row.value.removed) {
        for (const field of ['requestId','requestText','report_prompt','report_text']) delete draft[field];
        Object.assign(draft, restoreComposerDraftRecord(row.value), {revision:row.revision});
        if (legacy?.db) await Promise.all(draft.attachments.map(async item => {
          if (item.uploaded?.upload_id) return;
          const file = await new Promise((resolve, reject) => {
            const request = legacy.db.transaction('files').objectStore('files').get(uid + '\0' + item.id);
            request.onsuccess = () => resolve(request.result);
            request.onerror = () => reject(request.error);
          });
          if (file) {
            item.file = new File([file], item.file.name, {type:item.file.type,
              lastModified:item.file.lastModified});
            if (item.kind === 'image') item.preview = URL.createObjectURL(item.file);
          }
        }));
      } else if (draft.editVersion && !draft.savedVersion && row.value && !row.value.removed) {
        mergeEarlyComposerEdit(draft, restoreComposerDraftRecord(row.value), uid);
      }
      draft.storageError = '';
    } catch (error) {
      draft.storageError = '服务端草稿读取失败，当前输入保留：' + (error.message || error);
      draft.loadFailed = true;
    } finally {
      draft.loading = false;
      refreshComposerDraft(composerDraftOwner(uid)); syncComposerUnloadProtection();
    }
  })();
  composerHydrations.set(uid, task);
  return task;
}
/** Keystrokes landed before the first server read returned. The server text,
 *  attachments and quotes come first, the early input follows, and the merge
 *  replaces the queued save so the server never loses either side. */
function mergeEarlyComposerEdit(draft, server, uid) {
  if (server.text && !draft.text.startsWith(server.text)) {
    draft.text = server.text + (draft.text ? '\n' + draft.text : '');
  }
  draft.attachments = [...server.attachments,
    ...draft.attachments.filter(a => !server.attachments.some(b => b.id === a.id))];
  draft.quotes = [...server.quotes, ...draft.quotes.filter(q => !server.quotes.some(s => s.id === q.id))];
  draft.nextAttachmentNumber = Math.max(draft.nextAttachmentNumber || 1, server.nextAttachmentNumber || 1);
  if (!draft.session && server.session) draft.session = server.session;
  ensureComposerAttachmentNumbers(draft);
  queueComposerSave(draft, uid);
}
/** Replace the editor with a newer server row. Attachment objects this page
 *  already holds stay the same objects, so their File bytes, previews and
 *  in-flight uploads survive; only their server-side fields are refreshed. */
function adoptServerDraft(draft, row) {
  const next = restoreComposerDraftRecord(row.value);
  next.attachments = next.attachments.map(a => {
    const local = draft.attachments.find(b => a.id === b.id);
    if (!local) return a;
    local.number = a.number;
    if (a.kind) local.kind = a.kind;
    if (!local.uploaded?.upload_id && a.uploaded) local.uploaded = a.uploaded;
    return local;
  });
  for (const a of draft.attachments) if (a.preview && !next.attachments.some(b => a.id === b.id)) URL.revokeObjectURL(a.preview);
  for (const field of ['requestId','requestText','report_prompt','report_text']) delete draft[field];
  Object.assign(draft, next, {revision: row.revision, editVersion: draft.editVersion,
    savedVersion: draft.editVersion, storageError: ''});
  return draft;
}
let composerFollowBusy = false, composerFollowedAt = 0;
/** An idle page follows a newer server draft: the device that is typing wins
 *  and the others catch up. Nothing is adopted over unsaved local edits.
 *  `revision` is the server revision a poll reported; null forces a read,
 *  throttled to once a second. */
async function followServerDraft(uid, revision = null) {
  const owner = composerDraftOwner(uid), draft = composerDrafts.get(owner);
  if (!draft || !conversationSendEnabled() || draft.loading || draft.loadFailed || draft.handedOffSession
      || composerSending || composerFollowBusy || composerSaving.has(draft) || composerPendingSaves.has(draft)
      || draft.editVersion !== draft.savedVersion) return false;
  if (revision !== null && revision <= draft.revision) return false;
  if (revision === null && performance.now() - composerFollowedAt < 1000) return false;
  composerFollowBusy = true;
  const version = draft.editVersion;
  try {
    const row = await readServerComposerDraft(uid);
    composerFollowedAt = performance.now();
    if (!row || row.revision <= draft.revision || draft.editVersion !== version
        || composerSaving.has(draft) || composerSending) return false;
    adoptServerDraft(draft, row);
    refreshComposerDraft(owner); syncComposerUnloadProtection();
    return true;
  } catch {
    return false; // The next poll, focus or switch reads again.
  } finally {
    composerFollowBusy = false;
  }
}
function refreshComposerDraft(uid) {
  if (composerUid === uid) {
    const ta = $('#cinput'),text=composerDrafts.get(uid)?.text || ''; if (ta.value!==text) ta.value=text;
    renderComposerItems(); autoGrow(ta);
  }
  if (uid === BUG_REPORT_DRAFT_UID) {
    const ta = $('#bug-report-description'),text=composerDrafts.get(uid)?.text || ''; if (ta.value!==text) ta.value=text;
    renderBugReportItems(); autoGrow(ta);
  }
}
const composerPendingSaves=new Map();
const composerSaving=new Set();
function queueComposerSave(draft, uid) {
  composerPendingSaves.set(draft,{uid,version:++draft.editVersion,value:composerDraftRecord(draft,uid)});
  syncComposerUnloadProtection();
}
function persistComposerDraft(uid = composerUid) {
  uid = composerDraftOwner(uid);
  const draft = composerDrafts.get(uid);
  if (!draft) return Promise.resolve(false);
  if (typeof staleBuildShown !== 'undefined' && staleBuildShown) return Promise.resolve(false);
  queueComposerSave(draft, uid);
  if (composerSaving.has(draft)) return composerSaveQueues.get(draft);
  composerSaving.add(draft);
  const task=(async () => {
    await new Promise(resolve=>setTimeout(resolve,150));
    try {
      await hydrateComposerDraft(uid);
      if (!conversationSendEnabled() || draft.loadFailed) throw new Error(draft.storageError || '草稿保存未启用');
      while (composerPendingSaves.has(draft)) {
        const pending=composerPendingSaves.get(draft);composerPendingSaves.delete(draft);
        let data=await post('api/session/conversation',{uid:pending.uid,revision:draft.revision,value:pending.value});
        for (let attempt=0;data.code==='draft_revision' && attempt<2;attempt++) {
          // Another page saved first. This page is the one still editing, so
          // its input wins: rebase onto the server revision and save again.
          const row=await readServerComposerDraft(pending.uid);
          if (row.revision>draft.revision) draft.revision=row.revision;
          data=await post('api/session/conversation',{uid:pending.uid,revision:draft.revision,value:pending.value});
        }
        if (data.reload) throw new Error(data.error || '页面已更新，请重新加载后再提交');
        if (data.error) throw new Error(data.error);
        draft.revision=data.draft.revision;draft.savedVersion=pending.version;draft.storageError='';
      }
      return true;
    } catch (error) {
      draft.storageError='服务端草稿保存失败，当前输入保留：'+(error.message || error);
      return false;
    } finally {
      composerSaving.delete(draft);syncComposerUnloadProtection();
      const owner = composerDraftOwner(uid);
      if (owner === BUG_REPORT_DRAFT_UID) {
        renderSavedComposerInputs($('#bug-report-items'), draft);
      } else if (composerDraftOwner(composerUid) === owner) {
        renderSavedComposerInputs($('#compose-items'), draft);
      }
    }
  })();
  composerSaveQueues.set(draft,task);
  composerDraftWrites=Promise.all([...composerSaveQueues.values()]);
  return task;
}
const composerUnloadWarning = event => {event.preventDefault(); event.returnValue = '';};
let composerUnloadProtected = false;
function syncComposerUnloadProtection() {
  const pending = [...composerDrafts.values()].some(draft => draft.storageError
    || draft.editVersion > draft.savedVersion
    || draft.attachments.some(item => !item.uploaded?.upload_id && item.file instanceof Blob));
  if (pending === composerUnloadProtected) return;
  composerUnloadProtected = pending;
  if (pending) window.addEventListener('beforeunload', composerUnloadWarning);
  else window.removeEventListener('beforeunload', composerUnloadWarning);
}
function renderSavedComposerInputs(box, draft) {
  const existing = box.querySelector(':scope > .draft-save-error[role="alert"]');
  if (!draft.storageError) { existing?.remove(); return; }
  if (existing) { existing.textContent = draft.storageError; return; }
  const error = el('div', 'draft-save-error', draft.storageError);
  error.setAttribute('role','alert'); box.append(error);
}
const composerDraftRecoveryReady = Promise.resolve();

let composerServerRecovery;
function recoverServerComposerDrafts() {
  if (!conversationSendEnabled()) return Promise.resolve();
  if (composerServerRecovery) return composerServerRecovery;
  const task=(async () => {
    const nodes=HUB_MODE ? Nodes.list.map(n=>n.id) : [''];
    const results=await Promise.allSettled(nodes.map(async node=> {
      const url=appUrl('api/session/conversation/drafts' + (node?'?node='+node:''));
      const response=await fetch(url,{cache:'no-store'}),data=await response.json();
      if (!response.ok || data.error) throw new Error(data.error || `HTTP ${response.status}`);
      for (const row of data.drafts || []) {
        if (!row.uid || row.uid.startsWith('report:') || composerDrafts.has(row.uid)) continue;
        const draft=restoreComposerDraftRecord(row.draft.value);draft.revision=row.draft.revision;
        composerDrafts.set(row.uid,draft);
      }
    }));
    if (results.some(result=>result.status==='rejected')) composerServerRecovery=null;
    if (S.sig && typeof renderSide==='function') renderSide();
  })();
  composerServerRecovery=task;return task;
}

function deleteComposerDraftStorage(uid) {
  localStorage.removeItem(STORAGE_PREFIX + 'composerDraft.' + uid);
  const remaining = store.get('composerDraftUids', []).filter(key => key !== uid);
  if (remaining.length) store.set('composerDraftUids', remaining);
  else localStorage.removeItem(STORAGE_PREFIX + 'composerDraftUids');
}

/** Epoch seconds a pending row started: the receipt's `started`, else the
 *  row's own `created`, else `fallback`. A re-render must never invent a new
 *  one, or the row keeps changing its sidebar position. */
function pendingStartedAt(info, fallback = Date.now() / 1000) {
  const started = Number(info?.started);
  if (Number.isFinite(started) && started > 0) return started;
  const created = Date.parse(info?.created || '');
  return Number.isFinite(created) ? created / 1000 : fallback;
}

function rememberComposerSession(draft, info) {
  const previous = draft.session?.name === info.name ? Number(draft.session.started) : 0;
  draft.session = Object.fromEntries(['uid', 'name', 'source', 'cwd', 'node_id', 'node_name',
    'record_id', 'launch_id', 'instance_id', 'title', 'kind', 'report_id']
    .filter(key => info[key] != null).map(key => [key, info[key]]));
  // Once term/list drops the exited instance, the draft-retained sidebar row is
  // sorted by this stamp; keep the first one recorded for the same instance.
  const started = previous > 0 ? previous : pendingStartedAt(info, 0);
  if (started > 0) draft.session.started = started;
}

function syncComposerDraftBindings() {
  for (const [uid, draft] of [...composerDrafts]) {
    if (!uid.startsWith('tmux:') || !draft.session?.instance_id) continue;
    const row = (T.list || []).find(row => row.name === draft.session.name
      && row.instance_id === draft.session.instance_id && row.uid);
    if (row) migrateComposerDraft(uid, row.uid);
  }
}

function composerHistoryStamp(entry) {
  return `${entry?.end || 0}:${entry?.version?.head || ''}:${entry?.anchor || ''}`;
}

function nativeComposerHistory(messages) {
  return (messages || []).filter(message =>
    ['user', 'command'].includes(message?.role)
    && message.counted !== false && String(message.text || '').trim()
  ).map((message, index) => ({
    id: `native-${index}`, text: String(message.text), ts: message.ts || null,
  }));
}

function withQueuedComposerHistory(uid, items) {
  const result = items.map(item => ({ ...item }));
  for (const [index, item] of queuedMessages(uid).entries()) {
    if (!String(item?.text || '').trim()) continue;
    result.push({
      id: `queued-${item.id || index}`, text: String(item.text),
      ts: item.created || null,
    });
  }
  return result;
}

async function composerHistoryItems(uid) {
  const entry = cache.get(viewKey(uid));
  let items;
  if (entry && !entry.partial) {
    items = nativeComposerHistory(entry.msgs);
  } else {
    const stamp = composerHistoryStamp(entry);
    const cached = composerInputHistoryCache.get(uid);
    if (cached?.stamp === stamp) {
      items = cached.items.map(item => ({ ...item }));
    } else {
      const query = new URLSearchParams({uid});
      const response = await fetch(appUrl(`api/session/input-history?${query}`));
      const data = await response.json();
      if (!response.ok || data.error) throw new Error(data.error || `HTTP ${response.status}`);
      items = (data.history || []).map((item, index) => ({
        id: `native-${index}`, text: String(item.text || ''), ts: item.ts || null,
      })).filter(item => item.text.trim());
      const resultStamp = `${data.end || 0}:${data.version?.head || ''}:${data.anchor || ''}`;
      composerInputHistoryCache.set(uid, {
        stamp: resultStamp, items: items.map(item => ({ ...item })),
      });
    }
  }
  return withQueuedComposerHistory(uid, items);
}

function closeComposerHistory() {
  const box = $('#input-history');
  composerHistoryPicker.open = false;
  composerHistoryPicker.uid = null;
  composerHistoryPicker.seq++;
  box.classList.add('hidden');
  box.replaceChildren();
  $('#cinput').setAttribute('aria-expanded', 'false');
  $('#cinput').removeAttribute('aria-activedescendant');
}

function setComposerHistoryIndex(index) {
  const picker = composerHistoryPicker;
  if (!picker.open || !picker.items.length) return;
  picker.index = Math.max(0, Math.min(index, picker.items.length - 1));
  const box = $('#input-history');
  box.querySelectorAll('.input-history-item.selected').forEach(node => {
    node.classList.remove('selected');
    node.setAttribute('aria-selected', 'false');
  });
  const selected = box.querySelector(`[data-history-index="${picker.index}"]`);
  selected?.classList.add('selected');
  selected?.setAttribute('aria-selected', 'true');
  box.querySelector('.input-history-position').textContent =
    `${picker.index + 1} / ${picker.items.length}`;
  if (selected?.id) $('#cinput').setAttribute('aria-activedescendant', selected.id);
  selected?.scrollIntoView({block: 'nearest'});
}

function renderComposerHistory(state = 'ready') {
  const picker = composerHistoryPicker;
  const box = $('#input-history');
  box.replaceChildren();
  box.classList.remove('hidden');
  $('#cinput').setAttribute('aria-expanded', 'true');
  const head = el('div', 'input-history-head');
  head.appendChild(el('span', '', '输入历史'));
  const position = el('span', 'input-history-position',
    state === 'loading' ? '加载中…'
      : `${Math.max(0, picker.index + 1)} / ${picker.items.length}`
        + (state === 'refreshing' ? ' · 加载全部…' : ''));
  head.appendChild(position);
  box.appendChild(head);
  if (state === 'loading' || !picker.items.length) {
    box.appendChild(el('div', 'input-history-empty',
      state === 'loading' ? '正在加载输入历史…' : '暂无输入历史'));
    return;
  }
  const list = el('div', 'input-history-list');
  picker.items.forEach((item, index) => {
    const button = el('button', 'input-history-item');
    button.type = 'button';
    button.id = `input-history-option-${index}`;
    button.dataset.historyIndex = index;
    button.setAttribute('role', 'option');
    button.setAttribute('aria-selected', 'false');
    const text = el('span', 'input-history-text');
    text.textContent = item.text.slice(0, 600);
    const meta = document.createElement('small');
    meta.textContent = item.ts ? fmtTime(item.ts) : `${index + 1}`;
    button.append(text, meta);
    button.onmouseenter = () => setComposerHistoryIndex(index);
    button.onmousedown = event => event.preventDefault();
    button.onclick = () => {
      setComposerHistoryIndex(index);
      acceptComposerHistory();
    };
    list.appendChild(button);
  });
  box.appendChild(list);
  setComposerHistoryIndex(picker.index);
}

async function openComposerHistory() {
  const ta = $('#cinput');
  const uid = composerUid;
  if (!uid || composerSending || ta.value !== '') return;
  closeAttachMenu();
  const seq = ++composerHistoryPicker.seq;
  Object.assign(composerHistoryPicker, {
    open: true, uid, items: [], index: -1,
  });
  const entry = cache.get(viewKey(uid));
  const seed = withQueuedComposerHistory(uid, nativeComposerHistory(entry?.msgs));
  if (seed.length) {
    composerHistoryPicker.items = seed;
    composerHistoryPicker.index = seed.length - 1;
    renderComposerHistory(entry?.partial ? 'refreshing' : 'ready');
  } else {
    renderComposerHistory('loading');
  }
  try {
    const items = await composerHistoryItems(uid);
    if (!composerHistoryPicker.open || composerHistoryPicker.uid !== uid
        || composerHistoryPicker.seq !== seq || composerUid !== uid || ta.value !== '') return;
    const selected = composerHistoryPicker.items[composerHistoryPicker.index];
    composerHistoryPicker.items = items;
    const preserved = selected ? items.findLastIndex(item =>
      item.text === selected.text && item.ts === selected.ts) : -1;
    composerHistoryPicker.index = preserved >= 0 ? preserved : items.length - 1;
    renderComposerHistory();
  } catch (error) {
    if (!composerHistoryPicker.open || composerHistoryPicker.seq !== seq) return;
    composerHistoryPicker.items = [];
    composerHistoryPicker.index = -1;
    renderComposerHistory();
    $('#input-history .input-history-empty').textContent =
      `读取失败：${error.message || error}`;
  }
}

function acceptComposerHistory() {
  const picker = composerHistoryPicker;
  const item = picker.items[picker.index];
  if (!picker.open || !item) return false;
  const ta = $('#cinput');
  closeComposerHistory();
  ta.value = item.text;
  ta.dispatchEvent(new Event('input', {bubbles: true}));
  ta.focus();
  ta.setSelectionRange(ta.value.length, ta.value.length);
  return true;
}

function ensureComposerAttachmentNumbers(draft) {
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

function remapAttachmentReferences(text, remap) {
  if (!remap.size) return text;
  return String(text || '').replace(/\[附件([1-9]\d*)\]/g, (token, raw) => {
    const number = remap.get(Number(raw));
    return number ? `[附件${number}]` : token;
  });
}

function migrateComposerDraft(fromUid, toUid) {
  if (!fromUid || !toUid || fromUid === toUid) return;
  const draft = composerDrafts.get(fromUid);
  if (!draft) return;
  // This is only called after the terminal instance is explicitly bound.
  // Never concatenate an already edited destination with another draft.
  const target = composerDrafts.get(toUid);
  if (target?.editVersion && (target.text || target.attachments.length || target.quotes.length)) return;
  composerDrafts.set(toUid, draft);
  composerDraftAliases.set(fromUid, toUid);
  composerDrafts.delete(fromUid);
  if (composerHydrations.has(fromUid)) composerHydrations.set(toUid, composerHydrations.get(fromUid));
  for (const attachment of draft.attachments) {
    if (attachment.uploaded?.uid === fromUid) attachment.uploaded.uid = toUid;
  }
  if (composerUid === fromUid) composerUid = toUid;
}

function switchComposerDraft(uid) {
  const ta = $('#cinput');
  if (composerUid === uid) return;
  closeComposerHistory();
  composerUid = uid;
  const draft = composerDraft(uid, !!uid);
  if (draft) {draft.inputStatus = null; draft.inputProbe = (draft.inputProbe || 0) + 1;}
  ta.value = draft?.text || '';
  renderComposerItems();
  autoGrow(ta);
  if (uid) followServerDraft(uid); // Already hydrated: pick up edits saved elsewhere.
}

/** Persisted agents already drop the composer once the pane leaves the list.
 *  Unpersisted launches keep a `tmux:` uid after exit, so hide it explicitly. */
function sessionComposerEnded(uid = S.sel) {
  if (!uid) return false;
  if (typeof T !== 'undefined' && T.ended?.has(uid)) return true;
  const name = String(uid).startsWith('tmux:')
    ? String(uid).slice(5)
    : (typeof takenOver === 'function' ? takenOver(uid) : null);
  const view = name && typeof T !== 'undefined' ? T.views?.get(name) : null;
  if (view?.ended || view?.retired) return true;
  const rows = typeof T === 'undefined' ? [] : [
    ...(typeof pendingTmuxSessions === 'function' ? pendingTmuxSessions() : []),
    ...(T.pending || []),
  ];
  const row = rows.find(item => item.uid === uid
    || (typeof pendingUid === 'function' && pendingUid(item.name) === uid));
  return !!row && (row.state === 'exited' || row.state === 'failed' || row.stale);
}

function renderComposer() {
  const shell = typeof sessionIsPtyOnly === 'function' && sessionIsPtyOnly(S.sel);
  const enabled=conversationSendEnabled() || SessionDockCapabilities.allows('outbox') || shell;
  const name = enabled && sessionTerminalEnabled(S.sel) ? takenOver(S.sel) : null;
  const pending = enabled && String(S.sel || '').startsWith('tmux:');
  const show = !sessionComposerEnded(S.sel) && !!(name || pending);
  const box = $('#composer');
  box.classList.toggle('hidden', !show);
  $('#right')?.classList.toggle('shell-session', !!shell);
  switchComposerDraft(show ? S.sel : null);
  if (show) syncComposerMode();
  if (shell && typeof layoutTermPane === 'function') layoutTermPane();
}

function autoGrow(ta) {
  ta.style.height = 'auto';
  const wanted = ta.scrollHeight;
  ta.style.height = Math.min(180, Math.max(36, wanted)) + 'px';
  ta.style.overflowY = wanted > 180 ? 'auto' : 'hidden';
}

function syncComposerMode() {
  const ta = $('#cinput');
  ta.placeholder = MOBILE.matches
    ? '输入内容'
    : '输入内容，Enter 发送，Shift+Enter 换行';
  autoGrow(ta);
}

async function prepareTerminalDraft(uid) {
  const name = takenOver(uid);
  const cli = sessiondockCli(uid);
  if (!name || !['claude', 'codex'].includes(cli?.source) || uid.startsWith('tmux:')) {
    return { proceed: true, overwriteDraft: '' };
  }
  let d;
  try {
    d = await post('api/session/draft-status', { uid, name, ...termSendLease(name) });
  } catch (error) {
    alert('发送失败: ' + (error.message || error));
    return { proceed: false, overwriteDraft: '' };
  }
  if (d.error) {
    alert('发送失败: ' + d.error);
    return { proceed: false, overwriteDraft: '' };
  }
  if (!d.draft_conflict) return { proceed: true, overwriteDraft: '' };
  if (!confirmTerminalDraftOverwrite()) {
    return { proceed: false, overwriteDraft: '' };
  }
  return { proceed: true, overwriteDraft: d.draft_token || '' };
}

async function sendToSession(text, keys, uid = S.sel, media = [], options = {}) {
  const name = takenOver(uid);
  if (!name) return false;
  if ((text || options.requestId) && !keys && conversationSendEnabled()) {
    try {
      const draft = composerDraft(uid);
      const data = await post('api/session/conversation/send', {
        uid, name, text, request_id:options.requestId || crypto.randomUUID(),
        draft_revision:options.draftRevision, attachments:options.attachments || [],
        quotes:options.quotes || [], lease:termSendLease(name).lease || null,
      });
      if (data.error) {updateComposerInputStatus(uid, data); throw new Error(data.error);}
      acceptComposerServerRevision(draft,data.draft);
      S.live.add(uid); S.liveTmux.add(uid); S.lastSync = 0; S.syncGap = FAST_MIN;
      paintLive();
      return true;
    } catch (error) {
      alert('发送失败，输入保留：' + (error.message || error));
      return false;
    }
  }
  const cli = sessiondockCli(uid);
  const serverQueued = !!text && ['claude', 'codex'].includes(cli?.source)
    && !uid.startsWith('tmux:');
  const queuedId = text && !serverQueued && typeof queuePendingUserMessage === 'function'
    ? queuePendingUserMessage(uid, text, media) : null;
  let d;
  try {
    if (serverQueued) {
      const entry = cache.get(viewKey(uid));
      const activity = entry?.activity || null;
      const requestId = String(options.requestId || '')
        || globalThis.crypto?.randomUUID?.()
        || `${Date.now()}-${Math.random().toString(36).slice(2)}`;
      let overwriteDraft = String(options.overwriteDraft || '');
      for (let attempt = 0; attempt < 3; attempt++) {
        d = await post('api/session/send', {
          uid, name, text, media, activity, request_id: requestId,
          page_id: TERM_PAGE_ID,
          overwrite_draft: overwriteDraft,
          cursor: entry ? {
            start: entry.end, head: entry.version?.head, anchor: entry.anchor,
          } : null,
          ...termSendLease(name),
        });
        if (!d.draft_conflict) break;
        if (!confirmTerminalDraftOverwrite()) return false;
        overwriteDraft = d.draft_token || '';
      }
      if (d?.draft_conflict) {
        alert('终端草稿持续变化，消息未发送');
        return false;
      }
    } else {
      // Text on the raw path — a pending console before its first native
      // record, or a source without reliable send such as Grok — is
      // a bracketed paste, then Enter once the CLI took it.
      const rawText = !!text;
      const body = termInputBody(name, keys ? { name, keys, uid }
        : rawText ? { name, paste: text, uid } : { name, text });
      if (!body) {
        if (queuedId) discardQueuedUserMessage(uid, queuedId);
        alert('发送失败: 此后端未启用该会话的可靠发送；控制台键盘和快捷键仍可直接输入。');
        return false;
      }
      d = await post('api/term/send', body);
      if (rawText && !d.error) {
        // The CLI may briefly show a paste-burst marker. Sending Enter in
        // the same tick can be swallowed while that marker is active.
        await new Promise(resolve => setTimeout(resolve, 600));
        d = await post('api/term/send', termInputBody(name, { name, keys: ['Enter'], uid }));
      }
    }
  } catch (e) {
    if (queuedId) discardQueuedUserMessage(uid, queuedId);
    alert('发送失败: ' + (e.message || e));
    return false;
  }
  if (d.error) {
    if (queuedId) discardQueuedUserMessage(uid, queuedId);
    alert('发送失败: ' + d.error);
    return false;
  }
  if (serverQueued && typeof syncServerOutbox === 'function') {
    syncServerOutbox(uid, d.outbox || [], d.outbox_version);
  }
  if (Object.prototype.hasOwnProperty.call(d, 'activity')) {
    const entry = cache.get(viewKey(uid));
    if (entry) entry.activity = d.activity || null;
    if (S.sel === uid && !S.agent && typeof renderConversationTail === 'function') {
      renderConversationTail(entry?.activity || null, uid);
    }
  }
  S.live.add(uid);            // 发完立刻按最快节奏拉新消息
  S.liveTmux.add(uid);
  paintLive();
  S.syncGap = FAST_MIN;
  S.lastSync = 0;
  if (keys?.includes('Enter') && claudeRewinds.has(name)) {
    scheduleClaudeRewindSync(name);
  }
  return true;
}

function composerFileKind(file) {
  const prefix = String(file.type || '').split('/', 1)[0];
  return ['image', 'video', 'audio'].includes(prefix) ? prefix : 'file';
}

function composerKindIcon(kind) {
  return { image: '▧', video: '▶', audio: '♪', file: '⌑' }[kind] || '⌑';
}

function closeAttachMenu() {
  $('#attach-menu').classList.add('hidden');
  $('#cadd').classList.remove('on');
  $('#cadd').setAttribute('aria-expanded', 'false');
}

// 附件卡片同时服务对话输入框和缺陷报告框：两者的草稿结构、编号与上传流程一致。
function renderAttachmentCards(box, attachments,
  { onInsert, onRemove, onRetry = null, disabled = false, uid = null, render = () => {} }) {
  box.replaceChildren();
  for (const attachment of attachments) {
    const card = el('div', `draft-card ${attachment.status || ''}`);
    card.dataset.draftId = attachment.id;
    card.title = `点击插入 [附件${attachment.number}]`;
    card.onclick = e => {
      if (!e.target.closest('.draft-remove')) onInsert(attachment.number);
    };
    const thumb = el('span', 'draft-thumb');
    if (attachment.kind === 'image') loadStagedComposerPreview(attachment, uid, render);
    if (attachment.kind === 'image' && attachment.preview) {
      const image = document.createElement('img');
      image.src = attachment.preview;
      image.alt = '';
      thumb.appendChild(image);
    } else {
      thumb.textContent = composerKindIcon(attachment.kind);
    }
    const info = el('span', 'draft-info');
    const name = document.createElement('b');
    name.textContent = attachment.uploaded?.name || attachment.file?.name || 'attachment';
    const meta = document.createElement('small');
    const ref = `[附件${attachment.number}]`;
    const kindName = attachment.kind === 'file' ? '文件'
      : ({ image: '图片', video: '视频', audio: '音频' }[attachment.kind]);
    const summary = `${ref} · ${kindName} · ${fmtSize(attachment.file?.size ?? attachment.uploaded?.size ?? 0)}`;
    meta.textContent = attachment.status === 'uploading' ? `${summary} · ${attachment.progress || 0}%`
      : attachment.status === 'queued' ? `${summary} · 等待上传`
        : attachment.status === 'failed' ? `${summary} · ${attachment.error || '上传失败'}`
          : summary;
    info.append(name, meta);
    if (attachment.status === 'failed' && onRetry && attachment.file instanceof Blob) {
      const retry = el('button', 'draft-retry', '重试');
      retry.type = 'button'; retry.title = '重新上传';
      retry.disabled = disabled;
      retry.onclick = e => { e.stopPropagation(); onRetry(attachment); };
      info.appendChild(retry);
    }
    const remove = el('button', 'draft-remove', '×');
    remove.type = 'button';
    remove.title = remove.ariaLabel = attachment.cancelUpload ? '取消上传' : '移除附件';
    remove.disabled = disabled && !attachment.cancelUpload;
    remove.onclick = e => {
      e.stopPropagation();
      if (attachment.cancelUpload) attachment.cancelUpload();
      else onRemove(attachment.id);
    };
    card.append(thumb, info, remove);
    box.appendChild(card);
  }
}

// Normalize both structured CHECK responses and older nodes during a rollout.
// Only an explicit successful response can grant readiness.
function composerInputStatus(data) {
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

function composerInputAllowsSend(status) {
  return status?.state === 'ready';
}

function composerUsesInputStatus(uid = composerUid) {
  return conversationSendEnabled()
    && !(typeof sessionIsPtyOnly === 'function' && sessionIsPtyOnly(uid));
}

function updateComposerInputStatus(uid, data) {
  const owner = composerDraftOwner(uid), draft = composerDrafts.get(owner);
  if (!draft) return;
  draft.inputProbe = (draft.inputProbe || 0) + 1;
  const status = composerInputStatus(data);
  const changed = JSON.stringify(draft.inputStatus) !== JSON.stringify(status);
  draft.inputStatus = status;
  if (changed && composerDraftOwner(composerUid) === owner) renderComposerInputStatus();
}

async function probeComposerInput(uid) {
  const draft = composerDrafts.get(composerDraftOwner(uid)), name = takenOver(uid);
  if (!draft || !name) return {error:'会话尚未就绪，请切换终端检查'};
  const probe = (draft.inputProbe || 0) + 1;
  draft.inputProbe = probe;
  let data;
  try {
    data = await post('api/session/conversation/check', {uid, name,
      lease:termSendLease(name).lease || null});
  } catch (error) {
    data = {error:error.message || String(error)};
  }
  // An older poll cannot overwrite a newer SEND check, a switched view, or
  // the state of a replacement terminal using the same logical draft.
  if (draft.inputProbe === probe && takenOver(uid) === name) updateComposerInputStatus(uid, data);
  return data;
}

function syncComposerSendState() {
  const draft = composerDrafts.get(composerDraftOwner(composerUid));
  const blocked = composerUsesInputStatus()
    ? !composerInputAllowsSend(draft?.inputStatus) : !!activeCliQuestion(composerUid);
  $('#csend').disabled = (typeof staleBuildShown !== 'undefined' && staleBuildShown)
    || composerSending || !!draft?.loading || blocked;
}

const composerClaiming = new Set();
async function takeComposerTerminal(uid) {
  if (composerClaiming.has(uid) || composerUid !== uid || S.sel !== uid) return;
  composerClaiming.add(uid);
  renderComposerInputStatus();
  try {
    const name = takenOver(uid);
    if (!name) throw new Error('终端已不可用，请刷新会话状态');
    // Wait for an automatic attach already in flight before the explicit
    // force claim. The user click is the authorization to revoke its holder.
    const pending = T.views.get(name)?.attachPromise;
    if (pending) await pending;
    if (composerUid !== uid || S.sel !== uid) return;
    T.uid = uid;
    const opened = await openTermPane(name, true, MOBILE.matches ? null : 'full', false, true);
    if (!opened || !T.views.get(name)?.inputLease?.token) {
      throw new Error(ConsoleUI.errors.get(uid) || '未取得终端控制权');
    }
    await probeComposerInput(uid);
  } catch (error) {
    alert('接管终端失败：' + (error.message || error));
  } finally {
    composerClaiming.delete(uid);
    if (composerUid === uid) renderComposerInputStatus();
  }
}

function renderComposerInputStatus() {
  const node = $('#composer-input-status');
  const draft = composerDrafts.get(composerDraftOwner(composerUid));
  const status = draft && composerUsesInputStatus()
    && !composerInputAllowsSend(draft.inputStatus)
    ? draft.inputStatus || composerInputStatus(null) : null;
  const blocking = status && (status.state === 'blocked'
    || (status.state === 'unknown' && status.code !== 'input_check_pending'));
  node.classList.toggle('blocked', !!blocking);
  node.classList.toggle('hidden', !status);
  node.title = status?.message || '';
  node.replaceChildren();
  if (status) {
    node.append(el('span', 'composer-input-status-copy', status.message));
    if (status.code === 'terminal_ownership') {
      const uid = composerUid;
      const button = el('button', 'btn', composerClaiming.has(uid) ? '接管中…' : '接管');
      button.type = 'button';
      button.disabled = composerClaiming.has(uid);
      button.title = button.ariaLabel = '接管终端控制权';
      button.onclick = () => takeComposerTerminal(uid);
      node.append(button);
    }
  }
  syncComposerSendState();
}

async function reconcileComposerSubmission(uid) {
  const draft=composerDrafts.get(composerDraftOwner(uid));
  if (!draft?.requestId || draft.loading || draft.loadFailed || composerSending
      || composerSaving.has(draft) || draft.editVersion!==draft.savedVersion) return;
  const version=draft.editVersion,id=draft.requestId;
  const result=await priorComposerSubmission(uid,id);
  if (result?.state!=='sent' || !result.draft || result.draft.revision<draft.revision
      || draft.editVersion!==version || composerSaving.has(draft) || composerSending) return;
  // A later attachment may be saved as metadata while its bytes still live in
  // this page. A receipt refresh must preserve that File and its preview.
  adoptServerDraft(draft,result.draft);
  refreshComposerDraft(composerDraftOwner(uid));syncComposerUnloadProtection();
}
let composerInputProbeBusy=false;
setInterval(async () => {
  const uid=composerUid;
  if (!uid || !conversationSendEnabled() || document.hidden || composerSending || composerInputProbeBusy
      || $('#composer').classList.contains('hidden') || !takenOver(uid)) return;
  composerInputProbeBusy=true;
  try {
    const data = await probeComposerInput(uid);
    if (Number.isInteger(data.draft_revision)) await followServerDraft(uid,data.draft_revision);
  } catch { /* SEND independently checks the current input surface. */ }
  finally {
    try {await reconcileComposerSubmission(uid);} catch { /* A missing/in-progress receipt keeps the editor intact. */ }
    composerInputProbeBusy=false;
  }
},1500);

function renderComposerItems() {
  const box = $('#compose-items');
  const draft = composerDraft();
  if (!draft) {
    box.replaceChildren();
    renderComposerInputStatus();
    return;
  }
  renderAttachmentCards(box, draft.attachments, {
    uid: composerUid, render: renderComposerItems,
    disabled: composerSending,
    onRetry: attachment => stageComposerAttachment(attachment, composerUid),
    onInsert: insertComposerReference,
    onRemove: removeComposerAttachment,
  });
  for (const quote of draft.quotes) {
    const card = el('div', 'draft-card draft-quote');
    card.dataset.draftId = quote.id;
    const mark = el('span', '', '❝');
    const text = document.createElement('textarea');
    text.value = quote.text;
    text.maxLength = 16000;
    text.placeholder = '粘贴或输入要引用的文字';
    text.setAttribute('aria-label', '引用文字');
    text.oninput = () => { quote.text = text.value; persistComposerDraft(); };
    const remove = el('button', 'draft-remove', '×');
    remove.type = 'button';
    remove.title = remove.ariaLabel = '移除引用';
    remove.disabled = composerSending;
    remove.onclick = () => removeComposerQuote(quote.id);
    card.append(mark, text, remove);
    box.appendChild(card);
  }
  if (composerUid?.startsWith('tmux:') && !takenOver(composerUid)
      && ['exited','failed'].includes((T.pending || []).find(r=>pendingUid(r.name)===composerUid)?.state)
      && (draft.text || draft.attachments.length || draft.quotes.length)) {
    const restart=el('button','btn','重新启动');restart.type='button';
    restart.onclick=async () => {
      restart.disabled=true;
      try {
        draft.restartId ||= crypto.randomUUID();
        const uid=composerUid;
        const data=await post('api/session/conversation/restart',{uid,request_id:draft.restartId});
        if (data.error) throw new Error(data.error);
        const next=pendingUid(data.name);
        // The server has already bound both instances to the same logical draft.
        migrateComposerDraft(uid,next);composerHydrations.delete(next);
        await loadTermList();await openPendingSession(data);
      } catch (error){draft.storageError=error.message || String(error);renderComposerItems();}
    };
    box.append(restart);
  }
  renderSavedComposerInputs(box, draft, composerUid);
  $('#cinput').disabled = !!draft.loading;
  $('#cadd').disabled = (typeof staleBuildShown !== 'undefined' && staleBuildShown)
    || composerSending || !!draft.loading;
  renderComposerInputStatus();
}

function addComposerFiles(files) {
  const draft = composerDraft();
  if (!draft) return;
  const before = new Set(draft.attachments);
  addDraftFiles(draft, files);
  persistComposerDraft();
  for (const attachment of draft.attachments) {
    if (!before.has(attachment)) stageComposerAttachment(attachment, composerUid);
  }
  renderComposerItems();
}

function addDraftFiles(draft, files) {
  for (const file of files) {
    if (draft.attachments.length >= COMPOSER_MAX_FILES) {
      alert(`一次最多添加 ${COMPOSER_MAX_FILES} 个附件`);
      break;
    }
    if (!file.size || file.size > COMPOSER_MAX_FILE_BYTES) {
      alert(`「${file.name || '附件'}」为空或超过 512 MB`);
      continue;
    }
    const kind = composerFileKind(file);
    draft.attachments.push({
      id: `attachment-${globalThis.crypto?.randomUUID?.() || ++composerDraftSeq}`, number: draft.nextAttachmentNumber++, file, kind,
      preview: kind === 'image' ? URL.createObjectURL(file) : '',
      status: '', uploaded: null, error: '',
    });
  }
}

function clipboardAttachmentFiles(data) {
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

function clipboardDirectoryNames(data) {
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

function clipboardCsvFile(data, callback) {
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

function insertComposerReference(number, ta = $('#cinput')) {
  if (!ta) return;
  const token = `[附件${number}]`;
  ta.focus();
  const start = Number.isInteger(ta.selectionStart) ? ta.selectionStart : ta.value.length;
  const end = Number.isInteger(ta.selectionEnd) ? ta.selectionEnd : start;
  ta.setRangeText(token, start, end, 'end');
  ta.dispatchEvent(new Event('input', { bubbles: true }));
}

function removeDraftAttachment(draft, id) {
  const at = draft.attachments.findIndex(x => x.id === id);
  if (at < 0) return null;
  const [removed] = draft.attachments.splice(at, 1);
  if (removed.preview) URL.revokeObjectURL(removed.preview);
  if (removed.cancelUpload) removed.cancelUpload();
  return removed;
}

function removeComposerAttachment(id, draft = composerDraft()) {
  if (!draft || composerSending) return;
  const removed = removeDraftAttachment(draft, id);
  discardStagedAttachment(removed, persistComposerDraft());
  renderComposerItems();
}

function addComposerQuote(text = '') {
  const draft = composerDraft();
  if (!draft) return;
  if (draft.quotes.length >= 4) return alert('一次最多添加 4 段引用');
  draft.quotes.push({ id: `quote-${globalThis.crypto?.randomUUID?.() || ++composerDraftSeq}`, text: String(text).trim().slice(0, 16000) });
  persistComposerDraft();
  renderComposerItems();
  boxFocusLastQuote();
}

function boxFocusLastQuote() {
  requestAnimationFrame(() => {
    const nodes = document.querySelectorAll('#compose-items .draft-quote textarea');
    nodes[nodes.length - 1]?.focus();
  });
}

function removeComposerQuote(id, draft = composerDraft()) {
  if (!draft || composerSending) return;
  const at = draft.quotes.findIndex(x => x.id === id);
  if (at >= 0) draft.quotes.splice(at, 1);
  persistComposerDraft();
  renderComposerItems();
}

function buildComposerPrompt(text, attachments = [], quotes = []) {
  const attachmentPath = attachment => {
    // Use the destination node's convention, regardless of the browser OS.
    // Older nodes omit path_style; their absolute path still identifies Windows.
    const windows = attachment.path_style === 'windows'
      || (!attachment.path_style && /^(?:[a-z]:[\\/]|\\\\|\/\/)/i.test(attachment.path || ''));
    const relative = String(attachment.relative_path || '').replace(/^\.[\\/]/, '');
    if (!relative) return attachment.path;
    return windows ? `.\\${relative.replace(/\//g, '\\')}` : `./${relative}`;
  };
  const body = String(text || '');
  const quoted = quotes.map(x => String(x.text ?? x).trim()).filter(Boolean);
  if (!attachments.length && !quoted.length) return text;
  const blocks = [];
  if (attachments.length) {
    blocks.push(attachments.map((a, i) =>
      `附件${Number.isInteger(a.number) ? a.number : i + 1}: ${attachmentPath(a)}`).join('\n'));
  }
  if (quoted.length) blocks.push(quoted.map((q, i) => `引用${i + 1}:\n${q}`).join('\n'));
  let prompt = body;
  for (const block of blocks) {
    if (prompt) {
      const trailingNewlines = prompt.match(/\n*$/)?.[0].length || 0;
      prompt += '\n'.repeat(Math.max(0, 2 - trailingNewlines));
    }
    prompt += block;
  }
  return prompt;
}

async function uploadComposerAttachment(attachment, uid, attachmentId = null,
  {node = '', render = renderComposerItems} = {}) {
  if (attachment.uploaded?.upload_id && composerDraftOwner(attachment.uploaded.uid) === composerDraftOwner(uid)
      && (!node || attachment.uploaded.node === node || nodeOf(attachment.uploaded.uid) === node)) return attachment.uploaded;
  if (!(attachment.file instanceof Blob)) throw new Error('请重新选择未上传的附件：' + attachment.file.name);
  if (attachment.file.size > COMPOSER_MAX_FILE_BYTES) throw new Error('单个附件不能超过 512 MiB');
  attachment.status = 'uploading'; attachment.error = ''; attachment.progress = 0; render();
  const url = new URL(appUrl('api/session/conversation/attachment'));
  url.searchParams.set('uid', uid); url.searchParams.set('id', attachment.id);
  url.searchParams.set('name', attachment.file.name || 'attachment');
  if (node) url.searchParams.set('node', node);
  try {
    const data = await new Promise((resolve, reject) => {
      const xhr = new XMLHttpRequest();
      attachment.cancelUpload = () => xhr.abort();
      xhr.open('POST', url);
      xhr.setRequestHeader('Content-Type', attachment.file.type || 'application/octet-stream');
      xhr.upload.onprogress = event => {
        if (event.lengthComputable) attachment.progress = Math.floor(event.loaded / event.total * 100);
        render();
      };
      xhr.onload = () => {
        let data;
        try {data = JSON.parse(xhr.responseText);} catch {return reject(new Error(`HTTP ${xhr.status}`));}
        if (xhr.status < 200 || xhr.status >= 300 || data.error) reject(new Error(data.error || `HTTP ${xhr.status}`));
        else resolve(data);
      };
      xhr.onerror = () => reject(new Error('上传连接中断'));
      xhr.onabort = () => reject(new Error('上传已取消'));
      xhr.send(attachment.file);
    });
    attachment.uploaded = {...data, uid, node}; attachment.status = 'ready'; render();
    // Uploaded references become durable before any publication or SEND.
    if (!await persistComposerDraft(uid)) throw new Error('附件已上传，草稿引用保存失败');
    return attachment.uploaded;
  } catch (error) {
    attachment.status = 'failed'; attachment.error = error.message || String(error); render(); throw error;
  } finally {delete attachment.cancelUpload;}
}

const COMPOSER_UPLOAD_LANES = 2;
const composerUploadLanes = new Map();
/** Stage a new attachment's bytes right away, so a refresh or another device
 *  can still send it; publication into the session cwd waits for SEND. At most
 *  two uploads run per draft. A failed card keeps the File and offers a retry;
 *  SEND retries it too. */
function stageComposerAttachment(attachment, uid, options = {}) {
  const draft = composerDrafts.get(composerDraftOwner(uid));
  if (!draft || !conversationSendEnabled()) return Promise.resolve(null);
  if (attachment.staging) return attachment.staging;
  if (attachment.uploaded?.upload_id || !(attachment.file instanceof Blob)) return Promise.resolve(attachment.uploaded);
  const lane = composerUploadLanes.get(draft) || composerUploadLanes.set(draft, {queue: [], active: 0}).get(draft);
  const render = options.render || renderComposerItems;
  attachment.status = 'queued'; attachment.error = ''; render();
  attachment.staging = new Promise(resolve => lane.queue.push(async () => {
    try {
      if (!draft.attachments.includes(attachment)) { attachment.status = ''; resolve(null); return; }
      resolve(await uploadComposerAttachment(attachment, uid, null, options));
    } catch {
      resolve(null); // The failed card keeps the File; retry or SEND uploads again.
    } finally {
      delete attachment.staging;
    }
  }));
  pumpComposerUploads(lane);
  return attachment.staging;
}
function pumpComposerUploads(lane) {
  while (lane.active < COMPOSER_UPLOAD_LANES && lane.queue.length) {
    lane.active++;
    lane.queue.shift()().finally(() => { lane.active--; pumpComposerUploads(lane); });
  }
}
/** A draft read back from the server carries attachment metadata only: an
 *  image card has no local File for its thumbnail. Fetch the staged bytes once
 *  per card and keep the object URL on the attachment, so later renders reuse
 *  it and the existing removal paths revoke it. A card whose bytes are gone
 *  (published, or collected) keeps the kind icon instead of a broken image. */
const COMPOSER_PREVIEW_MAX_BYTES = 32 * 1024 * 1024;
function loadStagedComposerPreview(attachment, uid, render = () => {}) {
  if (!uid || attachment.kind !== 'image' || attachment.preview || attachment.previewLoading
    || Date.now() < (attachment.previewRetryAt || 0)) return;
  const id = attachment.uploaded?.upload_id;
  const size = attachment.file?.size ?? attachment.uploaded?.size ?? 0;
  if (!id || attachment.file instanceof Blob || size > COMPOSER_PREVIEW_MAX_BYTES) return;
  const url = new URL(appUrl('api/session/conversation/attachment'));
  url.searchParams.set('uid', uid); url.searchParams.set('id', id);
  attachment.previewLoading = true;
  (async () => {
    try {
      const response = await fetch(url, {cache:'no-store'});
      // Bytes that are gone stay gone; a transient failure may be retried.
      if (response.status === 404) attachment.previewRetryAt = Infinity;
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const blob = await response.blob();
      const live = composerDrafts.get(composerDraftOwner(uid))?.attachments.includes(attachment);
      // The card may be gone by now: never leak the object URL of a dropped one.
      if (live && !attachment.preview) attachment.preview = URL.createObjectURL(blob);
    } catch {
      attachment.previewRetryAt ||= Date.now() + 60000;
    } finally {
      delete attachment.previewLoading;
      render();
    }
  })();
}
/** Staged bytes of a removed attachment are released once the draft without
 *  it is saved; the server refuses while a draft or submission still names
 *  them, and the 24 h sweep covers a failed call. */
function discardStagedAttachment(attachment, saved = Promise.resolve(true)) {
  const id = attachment?.uploaded?.upload_id, uid = attachment?.uploaded?.uid;
  if (!id || !uid || !conversationSendEnabled()) return;
  Promise.resolve(saved)
    .then(ok => ok && post('api/session/conversation/attachment/discard', {uid, id}))
    .catch(() => {});
}

function composerAttachmentIdentity(uid) {
  const identity = {uid};
  if (SessionDockCapabilities.config.backend !== 'rust' || !String(uid).startsWith('tmux:')) {
    return identity;
  }
  const pending = (T.pending || []).find(row => pendingUid(row.name) === uid);
  if (pending?.record_id && pending?.instance_id) {
    identity.record_id = pending.record_id;
    identity.instance_id = pending.instance_id;
  }
  return identity;
}

let composerSending = false;
async function submitComposer() {
  if (!conversationSendEnabled() && !SessionDockCapabilities.allows('outbox')
      && !(typeof sessionIsPtyOnly === 'function' && sessionIsPtyOnly(composerUid || S.sel))) {
    alert('此服务尚未启用会话发送，请更新服务后重试'); return;
  }
  const ta = $('#cinput'), button = $('#csend'), add = $('#cadd');
  const uid = composerUid;
  let draft = composerDraft(uid);
  if (!draft || !takenOver(uid)) {
    alert('会话尚未就绪，输入已保留；可切换到终端检查启动状态'); return;
  }
  const text = ta.value, attachments = [...draft.attachments];
  const quotes = draft.quotes.map(x => ({id:x.id, text:x.text})).filter(x => x.text.trim());
  if (composerSending || (!text.trim() && !attachments.length && !quotes.length)) return;
  if (!conversationSendEnabled() || (typeof sessionIsPtyOnly === 'function' && sessionIsPtyOnly(uid))) {
    closeComposerHistory(); composerSending = true;
    button.disabled = true;
    try {
      const sent = await sendToSession(text, null, uid);
      if (sent) {
        ta.value = '';
        draft.text = '';
        // The typed text was saved while typing; clear the server copy too, or
        // the exited console would come back as a "retained draft" row.
        persistComposerDraft(uid);
      }
    } catch (error) {
      alert('发送失败，输入保留：' + (error.message || error));
    } finally {
      composerSending = false; button.disabled = false; autoGrow(ta);
    }
    return;
  }
  closeComposerHistory(); composerSending = true;
  let sendFailed = false;
  button.disabled = true; add.disabled = true;
  setSendButtonBusy(button, '发送中');
  renderComposerItems();
  try {
    draft.text = text;
    const priorPayload=JSON.stringify({text,attachments:attachments.map(a=>({upload_id:a.uploaded?.upload_id,number:a.number})),quotes});
    if (draft.requestId && (draft.requestText===priorPayload || (draft.report_prompt && draft.report_text===text))) {
      const previous=await priorComposerSubmission(uid,draft.requestId);
      if (previous?.state==='sent') {
        acceptComposerServerRevision(draft,previous.draft);
        await consumeComposerSubmission(uid,text,attachments,quotes);return;
      }
    }
    if (!await persistComposerDraft(uid)) throw new Error(draft.storageError || '草稿尚未保存');
    const check = await probeComposerInput(uid);
    const inputStatus = composerInputStatus(check);
    if (!composerInputAllowsSend(inputStatus)) throw new Error(inputStatus.message);
    const uploaded = [];
    for (let i = 0; i < attachments.length; i++) {
      setSendButtonBusy(button, `上传 ${i + 1}/${attachments.length}`);
      if (attachments[i].staging) await attachments[i].staging; // Staged on add; only a failure uploads here.
      uploaded.push({...await uploadComposerAttachment(attachments[i], uid), number:attachments[i].number});
    }
    const requestText = JSON.stringify({text, attachments:uploaded.map(a => ({upload_id:a.upload_id,number:a.number})), quotes});
    if (!(draft.report_prompt && draft.report_text===text && draft.requestId)
        && (draft.requestText !== requestText || !draft.requestId)) {
      draft.requestText = requestText; draft.requestId = crypto.randomUUID();
    }
    if (!await persistComposerDraft(uid)) throw new Error(draft.storageError || '提交标识尚未保存');
    const submittedRevision = draft.revision;
    setSendButtonBusy(button, '发送中');
    const sent = await sendToSession(text, null, uid, [], {requestId:draft.requestId,
      draftRevision:submittedRevision, attachments:uploaded, quotes});
    if (sent) await consumeComposerSubmission(uid,text,attachments,quotes);
    else sendFailed = true;
  } catch (error) {
    sendFailed = true;
    alert('发送失败，输入保留：' + (error.message || error));
  } finally {
    composerSending = false; setSendButtonBusy(button, ''); add.disabled = false;
    renderComposerItems(); autoGrow(ta);
    // A disabled Send button (including keyboard/touch activation) can leave
    // focus on <body> after the error dialog closes. Resume the retained draft
    // only if the user has not focused another control meanwhile.
    if (sendFailed && composerUid === uid && !ta.disabled
        && (document.activeElement === button || document.activeElement === document.body)) {
      ta.focus({preventScroll:true});
    }
  }
}

$('#cinput').addEventListener('input', e => {
  const draft = composerDraft();
  if (draft) { draft.text = e.target.value; persistComposerDraft(); }
  if (composerHistoryPicker.open && e.target.value !== '') closeComposerHistory();
  autoGrow(e.target);
});
$('#cinput').addEventListener('keydown', e => {
  if (e.isComposing) return;
  if (composerHistoryPicker.open) {
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      setComposerHistoryIndex(composerHistoryPicker.index - 1);
      return;
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setComposerHistoryIndex(composerHistoryPicker.index + 1);
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      acceptComposerHistory();
      return;
    }
    if (e.key === 'Escape') {
      e.preventDefault();
      closeComposerHistory();
      return;
    }
    if (!['Shift', 'Control', 'Alt', 'Meta'].includes(e.key)) closeComposerHistory();
  } else if (e.key === 'ArrowUp' && e.currentTarget.value === '') {
    e.preventDefault();
    openComposerHistory();
    return;
  }
  // 手机软键盘没有方便的 Shift+Enter：Enter 始终换行，只允许按钮发送。
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing && !MOBILE.matches) {
    e.preventDefault();
    submitComposer();
  }
});
MOBILE.addEventListener('change', () => {
  syncComposerMode();
  renderTakeoverBtn();
});
syncComposerMode();
// A mouse click submits the current draft; it must not move focus from the
// editor to a button that is disabled while SEND is in flight.
$('#csend').addEventListener('mousedown', event => {
  if (document.activeElement === $('#cinput')) event.preventDefault();
});
$('#csend').onclick = () => {
  submitComposer();
};
let composerEscAt = -Infinity;

function scheduleClaudeRewindSync(name, delay = 450) {
  const state = claudeRewinds.get(name);
  if (!state) return;
  clearTimeout(state.timer);
  state.timer = setTimeout(() => syncClaudeRewind(name), delay);
}

async function syncClaudeRewind(name) {
  const state = claudeRewinds.get(name);
  if (!state || state.syncing) return;
  state.syncing = true;
  try {
    const result = await post('api/session/rewind', {
      action: 'sync', uid: state.uid, name,
    });
    if (result.error) return;
    if (!result.pending) claudeRewinds.delete(name);
    if (result.changed) {
      // timeline pin 会改变 cursor 的逻辑叶子，即便 JSONL 一个字节都没变；
      // 用现有增量接口拿 reset，原子替换缓存和当前 DOM。
      S.lastSync = 0;
      await syncSession(state.uid);
    }
  } catch { /* 终端仍可继续使用；下一次 Enter 会重试同步 */ }
  finally {
    const current = claudeRewinds.get(name);
    if (current) current.syncing = false;
  }
}

async function revealNativeTerminal(uid = S.sel) {
  const name = takenOver(uid);
  if (!name || S.sel !== uid) return false;
  T.uid = uid;
  await openTermPane(name, true, MOBILE.matches ? null : 'full');
  return true;
}

function activeCliQuestion(uid) {
  const entry = cache.get(viewKey(uid));
  if (entry?.prompt?.questions?.length) return entry.prompt;
  const question = typeof pendingHistoryQuestion === 'function'
    ? pendingHistoryQuestion(entry) : null;
  return question ? {id: question.call_id, questions: question.questions} : null;
}

async function answerCliQuestion(uid, optionIndex) {
  const prompt = activeCliQuestion(uid);
  const rows = prompt?.questions;
  if (rows?.length !== 1 || rows[0].multiple || !rows[0].options?.[optionIndex]) return false;
  // 不同 CLI 的菜单定位语义不同（Claude 用方向键，Codex 用数字直选），
  // 具体按键必须由各自实现决定，不能在公共交互层猜测当前光标位置。
  const keys = sessiondockCli(uid)?.questionAnswerKeys(prompt, optionIndex);
  if (!keys?.length) return false;
  return sendToSession(null, keys, uid);
}

async function answerCliQuestionForm(uid, optionIndexes) {
  const prompt = activeCliQuestion(uid);
  const cli = sessiondockCli(uid);
  if (!cli?.canAnswerQuestionForm(prompt)) return false;
  const groups = cli.questionFormAnswerKeyGroups(prompt, optionIndexes);
  if (!groups?.length) return false;
  for (let i = 0; i < groups.length; i++) {
    if (!await sendToSession(null, groups[i], uid)) return false;
    // Enter 会让 Claude 卸载当前题并渲染下一题或 Review。分开发送并留出
    // 一个短事件循环间隔，避免后一题按键被旧题的输入处理器吞掉。
    if (i < groups.length - 1) {
      await new Promise(resolve => setTimeout(resolve, 50));
    }
  }
  return true;
}

async function cancelCliQuestion(uid) {
  composerEscAt = -Infinity;
  const keys = sessiondockCli(uid)?.questionCancelKeys(activeCliQuestion(uid));
  return keys?.length ? sendToSession(null, keys, uid) : false;
}

async function sendComposerEscape(now = performance.now()) {
  const uid = S.sel;
  const entry = cache.get(viewKey(uid));
  const visibleActivity = $('#activity')?.dataset.state;
  // activity 缓存可能来自上一个已结束的 Claude 进程；renderActivity 会把它
  // 隐藏。Esc 必须服从用户眼前的交互态，不能被这条旧 working 永久挡住回滚。
  const busy = !!entry?.prompt
    || !!pendingHistoryQuestion(entry)
    || ['working', 'waiting'].includes(visibleActivity);
  const draft = composerDraft(uid, false);
  const empty = !String($('#cinput')?.value || '').trim()
    && !(draft?.attachments?.length) && !(draft?.quotes?.some(q => q.text?.trim()));
  const escape = sessiondockCli(uid)?.repeatedEscape(now, composerEscAt, { busy, empty })
    || { rewind: false, nextAt: -Infinity };
  const rewind = escape.rewind;
  composerEscAt = escape.nextAt;
  const name = takenOver(uid);
  const sent = await sendToSession(null, ['Escape'], uid);
  if (!rewind || !sent || !name || S.sel !== uid) return sent;

  // 回滚点、恢复代码/对话的选项都由原生 CLI 自己维护。第二次 Esc 后直接
  // 揭示原生 TUI；这里只记住进入选择器前的叶子，最终选择仍服从原生菜单。
  try {
    const began = await post('api/session/rewind', {action: 'begin', uid, name});
    if (began.ok) claudeRewinds.set(name, {uid, timer: null, syncing: false});
  } catch { /* 记录失败不应阻止原生回滚 */ }
  await revealNativeTerminal(uid);
  return sent;
}
$('#cesc').onclick = () => sendComposerEscape();

$('#cadd').onclick = e => {
  e.stopPropagation();
  closeComposerHistory();
  const menu = $('#attach-menu');
  const open = menu.classList.toggle('hidden');
  $('#cadd').classList.toggle('on', !open);
  $('#cadd').setAttribute('aria-expanded', String(!open));
};
$('#attach-menu').onclick = e => {
  const button = e.target.closest('button[data-attach]');
  if (!button) return;
  const type = button.dataset.attach;
  closeAttachMenu();
  if (type === 'quote') {
    addComposerQuote(lastMessageSelectionUid === composerUid ? lastMessageSelection : '');
    lastMessageSelection = '';
    lastMessageSelectionUid = null;
    return;
  }
  const input = $('#cfile');
  input.accept = ATTACH_ACCEPT[type];
  input.dataset.kind = type;
  input.click();
};
$('#cfile').onchange = e => {
  addComposerFiles([...e.target.files]);
  e.target.value = '';
};
document.addEventListener('click', e => {
  if (!e.target.closest('.attach-picker')) closeAttachMenu();
  if (!e.target.closest('.composer-input-wrap')) closeComposerHistory();
});
document.addEventListener('selectionchange', () => {
  const selection = getSelection();
  if (!selection || selection.isCollapsed || !selection.anchorNode || !selection.focusNode) return;
  const messages = $('#msgs');
  if (messages?.contains(selection.anchorNode) && messages.contains(selection.focusNode)) {
    lastMessageSelection = selection.toString().trim().slice(0, 16000);
    lastMessageSelectionUid = S.sel;
  }
});
function pasteAttachmentFiles(e, addFiles) {
  const directories = clipboardDirectoryNames(e.clipboardData);
  const files = clipboardAttachmentFiles(e.clipboardData);
  if (directories.length) {
    e.preventDefault();
    if (files.length) addFiles(files);
    alert(`暂不支持直接粘贴文件夹：${directories.join('、')}。请先压缩后再粘贴。`);
    return;
  }
  if (!files.length) {
    // 表格软件偶尔只提供 text/csv 剪贴板项而不提供 File。此时保留其
    // 二进制附件语义；普通 text/plain 粘贴仍完全交给浏览器。
    if (!clipboardCsvFile(e.clipboardData, file => addFiles([file]))) return;
    e.preventDefault();
    return;
  }
  // 带附件的剪贴板常同时携带 text/plain；交给浏览器会把那份文字再粘贴一次。
  e.preventDefault();
  addFiles(files);
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

$('#cinput').addEventListener('paste', e => pasteAttachmentFiles(e, addComposerFiles));
bindFileDrop($('#composer'), addComposerFiles);

function setTermCtrl(on) {
  T.ctrlArmed = !!on;
  $('#termpane').classList.toggle('ctrl-locked', T.ctrlArmed);
  const b = $('[data-term-modifier="ctrl"]');
  if (b) {
    b.classList.toggle('on', T.ctrlArmed);
    b.setAttribute('aria-pressed', String(T.ctrlArmed));
  }
}

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

$('.term-keys').onclick = e => {
  const modifier = e.target.closest('[data-term-modifier]');
  if (modifier) {
    setTermCtrl(!T.ctrlArmed);
    T.term?.focus();
    return;
  }
  const b = e.target.closest('[data-term-key]');
  if (!b) return;
  const key = T.ctrlArmed ? `C-${b.dataset.termKey}` : b.dataset.termKey;
  setTermCtrl(false);
  sendToSession(null, [key]);
  T.term?.focus();
};

// ---- 高度拖动 ----
let tdrag = false;
let tdragPointer = null;
let tdragTopSnap = 48;
$('#tgrip').addEventListener('pointerdown', e => {
  tdrag = true;
  tdragPointer = e.pointerId;
  const composer = $('#composer');
  tdragTopSnap = Math.max(48, composer.offsetHeight || 0);
  e.currentTarget.setPointerCapture?.(e.pointerId);
  document.body.classList.add('dragging-v');
  e.preventDefault();
});
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
  }
  suspendTerm();
}
function foregroundTerm(force = false) {
  if (document.hidden || (!force && !termWasBackgrounded)) return;
  termWasBackgrounded = false;
  for (const view of T.views.values()) {
    if (view.webgl) {
      try { view.term.clearTextureAtlas(); } catch {}
    }
    if (view.resumeFocus) requestTermFocus(view, document.body);
    view.resumeFocus = false;
    reconnectTerm(view);
  }
}
document.addEventListener('visibilitychange', () => {
  if (document.hidden) backgroundTerm();
  else { foregroundTerm(); if (composerUid) followServerDraft(composerUid); }
});
addEventListener('focus', () => { if (composerUid) followServerDraft(composerUid); });
addEventListener('pagehide', backgroundTerm);
addEventListener('pageshow', e => foregroundTerm(e.persisted));
addEventListener('online', () => foregroundTerm(true));

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

loadTermList();
if (SessionDockCapabilities.config.backend === 'rust'
    && SessionDockCapabilities.allows('terminal') && !SessionDockCapabilities.allows('live'))
  setTimeout(pollRustTermList, 3000);
