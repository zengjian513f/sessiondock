'use strict';

// The compact project view is an opt-in, same-origin embedding of this page.
const PROJECT_VIEW = (() => {
  const input = new URLSearchParams(location.search).get('embedded_project') || '';
  const path = input.trim().replace(/\/+$/, '');
  let sameOriginFrame = false;
  try { sameOriginFrame = window.parent !== window && window.parent.location.origin === location.origin; } catch {}
  const cwd = sameOriginFrame && path.startsWith('/')
    && path.split('/').slice(1).every(part => part && part !== '.' && part !== '..') ? path : '';
  const match = row => {
    if (!cwd) return true;
    const path = String(row?.cwd || '').replace(/\/+$/, '');
    return path === cwd || path.startsWith(cwd + '/');
  };
  const rows = items => cwd ? items.filter(match) : items;
  const inheritPreference = key => /^(theme|font|newNode|newModel[\w]*\..+|agentStartMode\..+)$/.test(key);
  const error = text => cwd ? String(text).replaceAll(cwd, '当前项目') : text;

  if (cwd) {
    document.documentElement.classList.add('project-view');
    const toolbar = document.getElementById('project-view-tools');
    toolbar.hidden = false;
    toolbar.append(document.getElementById('new-session'));
    document.addEventListener('DOMContentLoaded', () => {
      window.parent.postMessage({type: 'sessiondock-project-ready'}, location.origin);
      document.addEventListener('keydown', event => {
        if (event.key !== 'Escape' || event.defaultPrevented
            || event.target.closest?.('input, textarea, select, [contenteditable], #xterm')
            || document.querySelector('dialog[open], :popover-open')) return;
        window.parent.postMessage({type: 'sessiondock-project-close'}, location.origin);
      });
    });
  }

  return {cwd, match, rows, inheritPreference, error};
})();
