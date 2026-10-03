import {useSessionUiStore} from '../../stores/session-ui'
import {consoleToast as showConsoleToast} from '../../migration/session-ui'
import {pendingUid} from '../../domain/runtime/pending'
import {useNodeStore,useConsoleStore} from '../../stores/runtime/nodes'

// The catalog and console presentation are owned here; terminal instances stay raw
// in their scoped terminal controller. Each imported operation is named explicitly.
export function createNodeService(pinia, environment, sessions, terminal, launch, sidebar, presentation) {
  const {HUB_MODE, STORAGE_PREFIX, capabilities, store, SOURCES, fetch, appUrl} = environment;
  const {catalog, selection, sessionHidden} = sessions;
  const {shortCwd, esc, uiIcon, appAlert} = presentation;
  const {canCompleteCwd, closeCwdPicker, commonSessionDirs, renderCommonCwdOptions} = launch;
  const {ensureSidebarVue, updateNodes} = sidebar;
  const ensureTerminalAssets = terminal.ensureAssets;
  const Nodes = useNodeStore(pinia),ui=useSessionUiStore(pinia).state;
  try { Nodes.off = new Set(JSON.parse(capabilities.stored('nodesOff', STORAGE_PREFIX)) || []); } catch {}
function nodeOf(uid) {
  return HUB_MODE ? String(uid || '').match(/^[^:]+:([a-f0-9]{32})~/)?.[1] || '' : '';
}
function nodeSelected(row) { return !HUB_MODE || !Nodes.off.has(row.node_id); }
function selectedNodeIds() { return Nodes.list.filter(n => !Nodes.off.has(n.id)).map(n => n.id); }
function nodeDirectory(row, length = 999) {
  return (row.node_name ? row.node_name + ' ' : '') + shortCwd(row.cwd || '(未知)', length);
}
// 机器配色由中央的注册表按机器配置，前端只负责显示。
function nodeColor(name) {
  return Nodes.list.find(n => n.name === name)?.color || '';
}
function newNodeId() { return HUB_MODE ? ui.newNode || '' : ''; }
function newDirsKey() { return HUB_MODE ? 'newDirs.' + newNodeId() : 'newDirs'; }
function newNodeCapabilities() { return HUB_MODE ? Nodes.capabilities[newNodeId()] || {} : terminal.state; }
function sessionTerminalEnabled(uid) {
  return typeof terminal.state !== 'undefined' && (HUB_MODE ? !!Nodes.capabilities[nodeOf(uid)]?.enabled : terminal.state.enabled);
}

const ConsoleUI = useConsoleStore(pinia);

function consoleUnavailableReason(uid, agent = null, lastError = true) {
  if (!uid) return '请先选择一个会话，再打开控制台。';
  if (agent) return '子代理没有独立控制台，请切换到主会话后打开控制台。';
  if (typeof terminal.state === 'undefined' || typeof terminal.takeover !== 'function')
    return '控制台组件尚未加载完成或加载失败，请稍后重试；持续失败时请刷新页面。';
  if (typeof ensureTerminalAssets !== 'function'
      && (!terminal.vendors.Terminal || !terminal.vendors.FitAddon))
    return '浏览器终端组件加载失败，无法显示控制台，请刷新页面重新加载。';
  if (ConsoleUI.busy.has(uid)) return '正在打开控制台，请等待当前连接请求完成。';
  if (terminal.state.listError) return terminal.state.listError;
  if (!terminal.state.listLoaded) return '正在读取控制台状态，请稍后重试。';
  const nid = nodeOf(uid), node = Nodes.list.find(n => n.id === nid);
  const cap = HUB_MODE ? Nodes.capabilities[nid] : terminal.state;
  if (HUB_MODE) {
    if (!node) return '会话所属机器尚未加载或已被移除，无法连接控制台。';
    const error = Nodes.errors.get('term')?.find(e => e.node_id === nid);
    if (error) return `${node.name} 终端列表请求失败：${error.error || '服务器未返回原因'}。`;
    if (!cap) return `${node.name} 的控制台状态尚未返回，请稍后重试。`;
  }
  if (typeof terminal.sessionRecordingReplayable === 'function' && terminal.sessionRecordingReplayable(uid))
    return '';
  if (capabilities.config.backend === 'rust' && terminal.state.ended?.has(uid)) {
    // An exited instance leaves the button as "接管会话"
    // whenever the source has a resume-capable CLI profile (the click starts
    // a fresh `--resume`); only an unresumable source keeps the gray
    // explanation. The exited xterm is never reclaimed automatically.
    // A shell recording is the console itself, so it must not go gray.
    const source = terminal.sessionTermMeta(uid)?.source || String(uid).split(':')[0];
    const resumable = !String(uid).startsWith('tmux:') && cap?.enabled
      && capabilities.allows('terminal_takeover') && !!cap?.resume_sources?.[source]
      && !terminal.linkedTermSession(uid, {followReplacement: true});
    if (!resumable) return terminal.state.ended.get(uid).reason;
  }
  if (capabilities.config.backend === 'rust') {
    const pending = terminal.state.pending?.find(row => row.record_id && pendingUid(row.name) === uid);
    if (pending?.stale) {
      const phase = typeof terminal.pendingPhase === 'function' ? terminal.pendingPhase(pending) : '';
      if (phase !== 'exited' && phase !== 'failed')
        return pending.unavailable_reason || '创建实例尚未就绪，不能连接控制台。';
    }
  }
  if (!cap?.enabled) return `${node ? node.name + '：' : ''}${cap?.unavailable_reason || '服务报告控制台不可用，但未返回具体原因。'}`;
  const linked = terminal.linkedTermSession(uid, {followReplacement: true});
  const source = terminal.sessionTermMeta(uid)?.source || String(uid).split(':')[0];
  if (!linked && !cap.sources?.[source])
    return `${node ? node.name + '：' : ''}未配置可用的 ${SOURCES[source]?.name || source} 启动命令。请检查该机器的 CLI 安装和启动器配置，再重启服务。`;
  // Rust: an unlinked session can only be resumed through an explicitly
  // configured resume-capable CLI profile; otherwise no name-based guessing.
  if (capabilities.config.backend === 'rust' && !linked
      && !(capabilities.allows('terminal_takeover')
        && cap?.resume_sources?.[terminal.sessionTermMeta(uid)?.source || String(uid).split(':')[0]]))
    return '该会话没有通过完整 UID 和实例校验的运行中终端；不能按名称猜测关联。';
  return lastError ? ConsoleUI.errors.get(uid) || '' : '';
}

function paintConsoleAvailability(button, uid, agent = null) {
  // 只有结构性不可用（离线机器 / 缺 CLI / 终端禁用 / 子代理 / 列表出错）才把按钮
  // 打成灰色并给出解释。上一次连接失败是可重试状态：按钮保持正常，点击直接重连，
  // 失败原因写进终端本身，不再用灰按钮 + 悬停问号 + 确认框拦住用户。
  const reason = consoleUnavailableReason(uid, agent, false);
  Object.assign(presentation.consoleButton,{unavailable:!!reason,...reason?{title:'',label:'控制台不可用：'+reason}:{}});
  if (button.matches(':hover') || document.activeElement === button)
    showConsoleToast(consoleUnavailableReason(uid, agent));
}

function applyNodeState(data, context = 'nodes') {
  if (!HUB_MODE || !data) return;
  if (Array.isArray(data.nodes)) Nodes.list = data.nodes;
  if (Array.isArray(data.machines)) Nodes.machines = data.machines;
  if (data.capabilities) Nodes.capabilities = data.capabilities;
  if (Array.isArray(data.errors)) Nodes.errors.set(context, data.errors);
  renderNodes();
}

function nodeClock(ts) {
  return new Date(ts * 1000).toLocaleString('zh-CN',
    {hour12: false, month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit'});
}
function nodeAgo(ts) {
  const s = Math.max(0, Math.round(Date.now() / 1000 - ts));
  if (s < 60) return `${s} 秒`;
  if (s < 3600) return `${Math.floor(s / 60)} 分钟`;
  if (s < 86400) return `${Math.floor(s / 3600)} 小时 ${Math.floor(s % 3600 / 60)} 分`;
  return `${Math.floor(s / 86400)} 天 ${Math.floor(s % 86400 / 3600)} 小时`;
}
function nodeOfflineReason(node) {
  if (node?.online !== false) return '';
  const fallback = [...Nodes.errors.values()].flat().find(e => e.node_id === node.id)?.error;
  const path = {'/api/live': '运行状态', '/api/sessions': '会话列表', '/api/term/list': '终端列表'}[node.failed_path];
  const lines = [`${node.name} 离线：${node.error || fallback || '中央站未能连接该机器，暂未收到具体错误原因。'}`];
  if (path) lines.push(`失败请求：${path}。`);
  if (node.offline_since) lines.push(`已离线 ${nodeAgo(node.offline_since)}（自 ${nodeClock(node.offline_since)}）。`);
  if (node.checked_at) lines.push(`上次检测：${nodeAgo(node.checked_at)}前；中央每 10 秒自动重试，机器恢复后会自动回到列表。`);
  if (node.last_seen) lines.push(`当前显示的是 ${nodeClock(node.last_seen)} 同步的离线缓存会话。`);
  return lines.join('\n');
}

function nodeRequestFailures(node) {
  if (!node) return [];
  const labels = {search: '全文搜索', sessions: '会话列表', live: '运行状态', term: '终端列表'};
  return ['live', 'term', 'sessions', 'search']
    .filter(context => (Nodes.errors.get(context) || []).some(row => row.node_id === node.id))
    .map(context => labels[context]);
}

function nodeChipReason(node) {
  const offline = nodeOfflineReason(node);
  if (offline) return offline;
  const failed = nodeRequestFailures(node);
  if (!failed.length) return '';
  return `${node.name} ${failed.join('、')}失败或超时。相关结果可能不完整或未更新。`;
}

// 顶栏压缩时机器 chip 只显示缩写：首字母；与其它机器首字母相同就用前两个字母。
function nodeAbbrs(nodes) {
  const lead = (name, n) => Array.from(String(name || '').trim()).slice(0, n).join('').toLowerCase();
  const shown = text => text.charAt(0).toUpperCase() + text.slice(1);
  return new Map(nodes.map(node => {
    const clash = nodes.some(other => other !== node && lead(other.name, 1) === lead(node.name, 1));
    return [node.id, shown(lead(node.name, clash ? 2 : 1))];
  }));
}

function renderNodes() {
  if (!HUB_MODE) return;
  const host = document.querySelector('#node-chips'); if (!host) return;
  ensureSidebarVue(); Nodes.visible=true;
  const abbrs = nodeAbbrs(Nodes.list), counts = new Map();
  for (const row of catalog.sessions) if (!sessionHidden(row)) counts.set(row.node_id, (counts.get(row.node_id) || 0) + 1);
  updateNodes(Nodes.list.map(n => ({key: n.id, label: n.name, count: counts.get(n.id) || 0,
    on: !Nodes.off.has(n.id), abbr: abbrs.get(n.id), color: n.color || '', offline: n.online === false,
    issue: n.online !== false && !!nodeChipReason(n), reason: nodeChipReason(n),
    title: '点击选择或取消；右键或长按只选这台机器'})));
}

async function loadNodes() {
  if (!HUB_MODE) return;
  const r = await fetch(appUrl('api/nodes'));
  if (!r.ok) throw new Error('无法读取机器列表');
  applyNodeState(await r.json());
}

function start(){if(HUB_MODE)void loadNodes().catch(()=>{})}

return { ConsoleUI, nodeOf, nodeSelected, selectedNodeIds, nodeDirectory, nodeColor, newNodeId, newDirsKey, newNodeCapabilities, sessionTerminalEnabled, consoleUnavailableReason, showConsoleToast, paintConsoleAvailability, applyNodeState, nodeClock, nodeAgo, nodeOfflineReason, nodeRequestFailures, nodeChipReason, nodeAbbrs, renderNodes, loadNodes, start };
}
