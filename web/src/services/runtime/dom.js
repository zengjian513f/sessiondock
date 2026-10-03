

import {SOURCES} from '../../domain/runtime/sources'

const $ = selector => document.querySelector(selector);
export function createDom({core,status,capabilities}) {
const el = (tag, cls, html) => {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (html != null) n.innerHTML = html;
  return n;
};

const esc = s => String(s ?? '').replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));

const icon = src => `<svg class="ico source-icon" data-source="${src}" aria-hidden="true" style="color:${SOURCES[src].color}"><use href="#${SOURCES[src].icon}"/></svg>`;

const liveStatusTitle = tmux => !capabilities.allows('live') ? '运行状态未知，尚未实现进程探测'
  : tmux ? '运行于受管终端' : '运行中';

const uiIcon = name => `<svg class="ui-icon" aria-hidden="true"><use href="#i-${name}"/></svg>`;
return {el,esc,icon,liveStatusTitle,uiIcon};
}
