'use strict';

// Optional development-backend contract. An ordinary Python-served page has
// no such meta tag and retains all existing behavior.
globalThis.AgentHubCapabilities = (() => {
  const meta = document.querySelector('meta[name="agenthub-capabilities"]');
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
    ? config.storage_namespace : config.backend === 'rust' ? 'sessiondock.' : '';
  // Batch 44 WP-F: a same-origin deployment that replaced the Python page still
  // holds every preference under Python's prefix (`agenthub.`, hub
  // `agenthub.hub.<path>.`). `stored(key)` reads `<prefix><key>` and, only when
  // the Rust namespace is set, falls back once to that Python key and copies
  // the value forward; the Python key itself is never written again.
  const legacyNamespace = () => {
    if (!namespace) return '';
    const hub = document.querySelector('meta[name="agenthub-mode"]')?.content === 'hub';
    return hub ? `agenthub.hub.${location.pathname}.` : 'agenthub.';
  };
  const stored = (key, prefix = namespace) => {
    const value = localStorage.getItem(prefix + key);
    if (value !== null) return value;
    const legacy = legacyNamespace();
    if (!legacy || legacy === prefix) return null;
    const previous = localStorage.getItem(legacy + key);
    if (previous !== null) {
      try { localStorage.setItem(prefix + key, previous); } catch { /* quota: still read it */ }
    }
    return previous;
  };
  return Object.freeze({config, allows, namespace, declared: !!meta, stored});
})();
