'use strict';

// Optional development-backend contract. An ordinary Python-served page has
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
      config = {read_only: true, live: false, outbox: false, audit: false,
        search: false, files: false, configuration_error: true};
    }
  }
  config = Object.freeze({...config});
  const allows = name => config[name] !== false;
  const namespace = typeof config.storage_namespace === 'string' && config.storage_namespace
    ? config.storage_namespace : 'sessiondock.';
  const stored = (key, prefix = namespace) => {
    return localStorage.getItem(prefix + key);
  };
  return Object.freeze({config, allows, namespace, declared: !!meta, stored});
})();
