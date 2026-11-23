'use strict';

const STALL_MS = 5000;
let config, context = {}, active = false, epoch = 0, taskPending = false;
let lastTask = 0, lastFrame = 0, lastTick = 0, timer = 0;
let incident = null, points = [], sequence = 0;
const outgoing = [];
let sending = false, retryAt = 0;

function reset(now) {
  epoch++;
  lastTask = lastFrame = lastTick = now;
  taskPending = false;
  incident = null;
}

async function deliver() {
  if (sending || !outgoing.length || !active || performance.now() < retryAt) return;
  sending = true;
  const payload = outgoing[0];
  const base = new URL('.', self.location.href);
  try {
    let endpoint = new URL('api/audit/browser', base);
    // The Hub groups scoped UIDs. A standalone page without a selected session
    // has no owner yet, so store its page-level receipt on an online node.
    if (config.hub && !/^[^:]+:[a-f0-9]{32}~/.test(payload.uid)) {
      let nid = config.node;
      if (!/^[a-f0-9]{32}$/.test(nid)) {
        const nodes = await fetch(new URL('api/nodes', base), {signal:AbortSignal.timeout(5000)});
        const inventory = await nodes.json();
        nid = inventory.nodes?.find(node => node.online)?.id;
      }
      if (!/^[a-f0-9]{32}$/.test(nid)) throw Error('no online audit destination');
      endpoint = new URL('api/nodes/' + nid + '/api/audit/browser', base);
    }
    const response = await fetch(endpoint, {method:'POST', headers:{'Content-Type':'application/json'},
      body:JSON.stringify(payload), signal:AbortSignal.timeout(10000)});
    await response.arrayBuffer();
    if (!response.ok) throw Error('audit unavailable');
    if (outgoing[0] === payload) outgoing.shift();
  } catch { retryAt = performance.now() + 5000; }
  finally { sending = false; }
}

function record(event, now) {
  const data = {incident:incident.id, surface:config.surface, ...incident.context,
    missed:incident.missed, last_task_at:new Date(performance.timeOrigin + lastTask).toISOString(),
    last_frame_at:new Date(performance.timeOrigin + lastFrame).toISOString(),
    task_gap_ms:Math.round(now-lastTask), frame_gap_ms:Math.round(now-lastFrame),
    elapsed_ms:Math.round(now-incident.start), points:points.slice()};
  const uid = incident.context.uid || '';
  outgoing.push({page_id:config.pageId, _build:config.build, uid,
    events:[{event:'watchdog.'+event, ts:new Date().toISOString(), uid,
      severity:event === 'stalled' ? 'warning' : 'info', data}]});
  if (outgoing.length > 16) outgoing.shift();
  void deliver();
}

function check() {
  const now = performance.now();
  // A suspended worker cannot distinguish machine sleep from a blocked page.
  if (now-lastTick > STALL_MS) reset(now);
  lastTick = now;
  const stalled = now-lastTask >= STALL_MS || now-lastFrame >= STALL_MS;
  if (stalled && !incident) {
    incident = {id:++sequence, start:Math.min(lastTask,lastFrame), context:{...context},
      missed:now-lastTask >= STALL_MS ? 'task' : 'animation_frame'};
    record('stalled', now);
  } else if (!stalled && incident) {
    record('recovered', now);
    incident = null;
  }
  if (!taskPending) {
    taskPending = true;
    postMessage({type:'probe', epoch});
  }
  void deliver();
}

self.onmessage = ({data}) => {
  if (data.type === 'init') { config = data; return; }
  if (data.type === 'active') {
    clearInterval(timer);
    active = data.active;
    context = data.context;
    points = [];
    reset(performance.now());
    if (active) { timer = setInterval(check, 1000); check(); }
    return;
  }
  if (!active || (data.epoch !== undefined && data.epoch !== epoch)) return;
  if (data.context) context = data.context;
  if (data.points?.length) points = points.concat(data.points).slice(-16);
  if (data.type === 'task') { lastTask = performance.now(); taskPending = false; }
  if (data.type === 'frame') lastFrame = performance.now();
};
