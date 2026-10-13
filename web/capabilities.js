'use strict';

// Optional development-backend contract. An ordinary page has
// no such meta tag and retains all existing behavior.
globalThis.SessionDockCapabilities = (() => {
  const meta = document.querySelector('meta[name="sessiondock-capabilities"]');
  let config = {};
  if (meta) {
    try {
      const value = JSON.parse(meta.content);
      if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid capabilities');
      config = value;
    } catch {
      // A malformed declaration must not start unsupported background work.
      config = {read_only: true, live: false, audit: false,
        search: false, files: false, configuration_error: true};
    }
  }
  config = Object.freeze({...config});
  const allows = name => config[name] !== false;
  // Integrations are opt-in even on pages without a capability declaration.
  const integration = name => {
    const flag = {labdesk: 'research', filedock: 'file_browser'}[name];
    const value = config[name + '_url'];
    if (!flag || config[flag] !== true || typeof value !== 'string' || !value.trim()) return null;
    try {
      const url = new URL(value, location.href);
      if (!['http:', 'https:'].includes(url.protocol)) return null;
      url.pathname = url.pathname.replace(/\/*$/, '/');
      url.search = ''; url.hash = '';
      return url;
    } catch { return null; }
  };
  const namespace = typeof config.storage_namespace === 'string' && config.storage_namespace
    ? config.storage_namespace : 'sessiondock.';
  const stored = (key, prefix = namespace) => {
    return localStorage.getItem(prefix + key);
  };
  return Object.freeze({config, allows, integration, namespace, declared: !!meta, stored});
})();
