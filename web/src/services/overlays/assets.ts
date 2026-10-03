

// Load optional renderers only when their surface is opened. Failed loads can
// be retried, and every caller shares the same in-flight request.
export const assets = (() => {
  const base = new URL('.', location.href);
  const version = document.querySelector('meta[name="sessiondock-build"]')?.getAttribute("content");
  const pending = new Map<string, Promise<void>>();
  const url = (path: string) => {
    const value = new URL(path, base);
    if (version) value.searchParams.set('v', version);
    return value.href;
  };
  function load(path: string, css = false) {
    if (pending.has(path)) return pending.get(path);
    const promise = new Promise<void>((resolve, reject) => {
      const node = document.createElement(css ? 'link' : 'script');
      if (css) { (node as HTMLLinkElement).rel = 'stylesheet'; (node as HTMLLinkElement).href = url(path); }
      else { (node as HTMLScriptElement).src = url(path); (node as HTMLScriptElement).async = true; }
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
  return {url, script: (path: string) => load(path), style: (path: string) => load(path, true)};
})();

export const ensureTerminalAssets = async (grid: boolean) => {
  if (grid) return;
  await Promise.all([
    assets.style('vendor/xterm.css'),
    assets.script('vendor/xterm.js'),
  ]);
  await Promise.all(['addon-fit.js', 'addon-unicode11.js', 'addon-webgl.js']
    .map(name => assets.script(`vendor/${name}`)));
};

export const assetUrl = assets.url
export const loadScript = assets.script
export const loadStyle = assets.style
