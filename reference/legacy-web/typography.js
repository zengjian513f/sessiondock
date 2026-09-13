'use strict';
(() => {
  const choices = {
    ubuntu: '"SessionDock CJK Sans", "SessionDock Ubuntu Sans Mono", "Ubuntu Sans Mono", "SessionDock Cascadia Mono", "Cascadia Mono", "Adwaita Mono", "Ubuntu Mono", Consola, Consolas, sans-serif',
    cascadia: '"SessionDock CJK Sans", "SessionDock Cascadia Mono", "Cascadia Mono", "Adwaita Mono", "Ubuntu Mono", Consola, Consolas, sans-serif',
    system: '"SessionDock CJK Sans", ui-monospace, "SFMono-Regular", "Cascadia Mono", "Adwaita Mono", "Ubuntu Mono", "Liberation Mono", Consolas, sans-serif',
    consolas: '"SessionDock CJK Sans", Consolas, Consola, "Cascadia Mono", "Liberation Mono", sans-serif',
  };
  const root = new URL('.', location.href).pathname;
  const hub = document.querySelector('meta[name="sessiondock-mode"]')?.content === 'hub';
  const prefix = hub ? `sessiondock.hub.${root}.` : 'sessiondock.';
  function apply() {
    let choice = 'ubuntu';
    try { choice = JSON.parse(localStorage.getItem(prefix + 'font')) || choice; } catch {}
    document.documentElement.style.setProperty('--terminal-font', choices[choice] || choices.ubuntu);
  }
  window.SessionDockTypography = {choices, apply};
  apply();
  addEventListener('storage', event => { if (event.key === prefix + 'font' || event.key === null) apply(); });
})();
