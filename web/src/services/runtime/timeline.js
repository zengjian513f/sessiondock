const $ = selector => document.querySelector(selector);

import * as Sidebar from '../../migration/sidebar'

export function createTimeline({core,dom,sidebarView}) {
const timelinePath = cwd => (cwd || '(未知)').replace(/^\/home\/[^/]+/, '~').replace(/\/+$/, '') || '/';

const TIMELINE_COLORS = ['blue', 'teal', 'violet', 'amber', 'rose', 'olive', 'rust', 'cyan'];

let timelineColors = new Map();

function timelineDirectoryColors(rows) {
  const counts = new Map(), seen = new Set();
  for (const row of rows) {
    if (!row.cwd || row.cwd === '(未知)' || seen.has(row.uid)) continue;
    seen.add(row.uid); // Search results also contain sessions in the main list.
    const path = timelinePath(row.cwd);
    counts.set(path, (counts.get(path) || 0) + 1);
  }
  const common = [...counts].filter(([, count]) => count >= 2)
    .sort(([a, ac], [b, bc]) => bc - ac || (a < b ? -1 : a > b ? 1 : 0))
    .slice(0, TIMELINE_COLORS.length).map(([path]) => path);
  const assigned = new Map(common.filter(path => timelineColors.has(path))
    .map(path => [path, timelineColors.get(path)]));
  const free = TIMELINE_COLORS.filter(color => ![...assigned.values()].includes(color));
  for (const path of common) {
    if (!assigned.has(path)) assigned.set(path, free.shift());
  }
  // Preserve slots when session counts reorder the common directories. Filters
  // use the full pool, and reloads reuse the assignment instead of recoloring it.
  if (assigned.size !== timelineColors.size
      || [...assigned].some(([path, color]) => timelineColors.get(path) !== color)) {
    timelineColors = assigned;
    core.preferences.set('timelineDirectoryColors', [...assigned]);
  }
  return timelineColors;
}

let timelinePlanCache = {paths: new Set(), plans: new Map()};

function timelinePathPlans(rows) {
  const pathSet = new Set(rows.map(s => timelinePath(s.cwd)));
  if (pathSet.size === timelinePlanCache.paths.size
      && [...pathSet].every(path => timelinePlanCache.paths.has(path))) return timelinePlanCache.plans;
  const paths = [...pathSet];
  const frequency = new Map(), peers = new Map();
  for (const path of paths) {
    const parts = path.split('/'), leaf = parts.at(-1);
    for (const part of new Set(parts)) frequency.set(part, (frequency.get(part) || 0) + 1);
    // Any matching abbreviation must contain every literal component. Index
    // those components so same-basename temporary directories do not require
    // an all-pairs regex scan (their distinguishing component is often unique).
    if (!peers.has(leaf)) peers.set(leaf, new Map());
    const components = peers.get(leaf);
    for (const part of new Set(parts)) {
      if (!components.has(part)) components.set(part, new Set());
      components.get(part).add(path);
    }
  }
  const plans = new Map(paths.map(path => {
    const parts = path === '/' ? ['/'] : path.split('/');
    const leaf = parts.at(-1), hidden = new Set(), labels = [path];
    // Repeated ancestors carry less information. Ties favor keeping the deeper
    // context. Never remove the root anchor or any part of the final directory.
    const order = parts.map((_, i) => i).slice(1, -1).sort((a, b) =>
      frequency.get(parts[b]) - frequency.get(parts[a]) || a - b);
    for (const i of order) {
      hidden.add(i);
      const tokens = parts.flatMap((part, j) => hidden.has(j)
        ? (hidden.has(j - 1) ? [] : [null]) : [part]);
      // An ellipsis represents whole directories. If it could also stand for
      // another path with the same basename, retain the distinguishing ancestor.
      const components = peers.get(leaf);
      const candidates = tokens.filter(token => token !== null)
        .map(token => components.get(token)).reduce((a, b) => a.size <= b.size ? a : b);
      const pattern = candidates.size > 1 ? new RegExp('^' + tokens.map(token => token === null
        ? '(?:[^/]+/)*[^/]+' : token.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('/') + '$') : null;
      if (pattern && [...candidates].some(other => other !== path && pattern.test(other))) {
        hidden.delete(i);
        continue;
      }
      const label = tokens.map(token => token === null ? '…' : token).join('/');
      labels.push(label);
    }
    return [path, {leaf, labels}];
  }));
  timelinePlanCache = {paths: pathSet, plans};
  return plans;
}

let timelineFitContext = null;

function fitTimelineDirectories(elements = null, context = null) {
  if (core.state.sidebar.view !== 'date') return;
  elements ||= [...document.querySelectorAll('#side .cwd-path')];
  if (!elements.length) return;
  if (!context) {
    const rows = [...core.state.catalog.sessions, ...core.pending.pendingTmuxSessions(), ...(core.state.search.results || [])];
    context = timelineFitContext = {plans: timelinePathPlans(rows), colors: timelineDirectoryColors(rows)};
  }
  const {plans, colors} = context;
  const measure = document.createElement('canvas').getContext('2d');
  const font = getComputedStyle(elements[0]);
  measure.font = `${font.fontWeight} ${font.fontSize} ${font.fontFamily}`;
  const widths = new Map();
  // Batch layout reads before writes; repeated rows share measured labels.
  const updates = elements.map(element => {
    const plan = plans.get(element.dataset.path);
    if (!plan) return null;
    const color = colors.get(element.dataset.path) || '';
    const width = element.clientWidth;
    if (!width) return {element, color}; // Fit a closed date group when opened.
    const measured = label => {
      if (!widths.has(label)) {
        widths.set(label, measure.measureText(label).width);
      }
      return widths.get(label);
    };
    const label = plan.labels.find(label => measured(label) <= width)
      || plan.labels.reduce((best, label) => measured(label) < measured(best) ? label : best);
    return {element, color, label};
  });
  if (sidebarView().sidebarVueMounted) Sidebar.fitPaths(updates.filter(Boolean).map(update => ({
    key: update.element.closest('.item').dataset.key, color: update.color, label: update.label,
  })));
}

let timelineFitFrame = 0;

function scheduleTimelineFit() {
  cancelAnimationFrame(timelineFitFrame);
  timelineFitFrame = requestAnimationFrame(() => controller.fitTimelineDirectories());
}

const dayKey = iso => {
  const d = new Date(iso), p = n => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
};
function start(){
try {
  for (const [path, color] of core.preferences.get('timelineDirectoryColors', [])) {
    if (typeof path === 'string' && TIMELINE_COLORS.includes(color)
        && ![...timelineColors.values()].includes(color)) timelineColors.set(path, color);
  }
} catch { /* Ignore invalid saved assignments. */ }
new ResizeObserver(scheduleTimelineFit).observe($('#side'));
document.fonts.ready.then(scheduleTimelineFit);
}
const controller = {timelinePath,TIMELINE_COLORS,get timelineColors(){return timelineColors},timelineDirectoryColors,get timelinePlanCache(){return timelinePlanCache},timelinePathPlans,get timelineFitContext(){return timelineFitContext},fitTimelineDirectories,get timelineFitFrame(){return timelineFitFrame},scheduleTimelineFit,dayKey,start};
return controller;
}
