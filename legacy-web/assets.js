'use strict';

// Load optional renderers only when their surface is opened. Failed loads can
// be retried, and every caller shares the same in-flight request.
globalThis.SessionDockAssets = (() => {
  const base = new URL('.', location.href);
  const version = document.querySelector('meta[name="sessiondock-build"]')?.content;
  const pending = new Map();
  const url = path => {
    const value = new URL(path, base);
    if (version) value.searchParams.set('v', version);
    return value.href;
  };
  function load(path, css = false) {
    if (pending.has(path)) return pending.get(path);
    const promise = new Promise((resolve, reject) => {
      const node = document.createElement(css ? 'link' : 'script');
      if (css) { node.rel = 'stylesheet'; node.href = url(path); }
      else { node.src = url(path); node.async = true; }
      node.onload = () => resolve();
      node.onerror = () => {
        pending.delete(path);
        node.remove();
        reject(new Error(`无法加载 ${path}`));
      };
      document.head.appendChild(node);
    });
    pending.set(path, promise);
    return promise;
  }
  return {url, script: path => load(path), style: path => load(path, true)};
})();

globalThis.ensureTerminalAssets = async grid => {
  if (grid) return;
  await Promise.all([
    SessionDockAssets.style('vendor/xterm.css'),
    SessionDockAssets.script('vendor/xterm.js'),
  ]);
  await Promise.all(['addon-fit.js', 'addon-unicode11.js', 'addon-webgl.js']
    .map(name => SessionDockAssets.script(`vendor/${name}`)));
};
