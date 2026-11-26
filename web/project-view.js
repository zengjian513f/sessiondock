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
  const narrow = matchMedia('(max-width: 720px)');

  function setListOpen(open, restoreFocus = true) {
    if (!cwd || (open && !narrow.matches)
        || document.body.classList.contains('project-list-open') === open) return;
    const toggle = document.getElementById('project-list-toggle');
    document.body.classList.toggle('project-list-open', open);
    document.getElementById('right').inert = open;
    document.getElementById('project-list-scrim').hidden = !open;
    const close = document.getElementById('project-list-close');
    close.hidden = !open;
    toggle?.setAttribute('aria-expanded', String(open));
    if (open) close.focus();
    else if (restoreFocus) toggle?.focus({preventScroll: true});
  }

  function bindListButton(button, fallback) {
    if (!cwd) { button.onclick = fallback; return; }
    button.id = 'project-list-toggle';
    button.innerHTML = '<svg class="ui-icon" aria-hidden="true"><use href="#i-list"/></svg>';
    button.title = button.ariaLabel = '会话列表';
    button.setAttribute('aria-controls', 'left');
    button.setAttribute('aria-expanded', String(document.body.classList.contains('project-list-open')));
    button.onclick = () => setListOpen(true);
  }

  if (cwd) {
    document.documentElement.classList.add('project-view');
    const toolbar = document.getElementById('project-view-tools');
    toolbar.hidden = false;
    toolbar.append(document.getElementById('new-session'));
    document.getElementById('project-list-close').onclick = () => setListOpen(false);
    document.getElementById('project-list-scrim').onclick = () => setListOpen(false);
    narrow.addEventListener('change', event => {
      if (!event.matches) setListOpen(false, false);
    });
    document.addEventListener('DOMContentLoaded', () => {
      window.parent.postMessage({type: 'sessiondock-project-ready'}, location.origin);
      document.addEventListener('keydown', event => {
        if (event.key !== 'Escape' || event.defaultPrevented
            || document.querySelector('dialog[open], :popover-open')) return;
        if (document.body.classList.contains('project-list-open')) {
          event.preventDefault();
          setListOpen(false);
          return;
        }
        if (event.target.closest?.('input, textarea, select, [contenteditable], #xterm')) return;
        window.parent.postMessage({type: 'sessiondock-project-close'}, location.origin);
      });
    });
  }

  return {cwd, match, rows, inheritPreference, error, bindListButton, setListOpen};
})();
