

// Optional development-backend contract. An ordinary page has
// no such meta tag and retains all existing behavior.
export const capabilities = (() => {
  const meta = document.querySelector('meta[name="sessiondock-capabilities"]');
  let config: Record<string, unknown> = {};
  if (meta) {
    try {
      const value = JSON.parse(meta.getAttribute("content") || "");
      if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid capabilities');
      config = value;
    } catch {
      // A malformed declaration must not start unsupported background work.
      config = {read_only: true, live: false, outbox: false, audit: false,
        search: false, files: false, configuration_error: true};
    }
  }
  config = Object.freeze({...config});
  const allows = (name: string) => config[name] !== false;
  const namespace = typeof config.storage_namespace === 'string' && config.storage_namespace
    ? config.storage_namespace : 'sessiondock.';
  const stored = (key: string, prefix = namespace) => {
    return localStorage.getItem(prefix + key);
  };
  return Object.freeze({config, allows, namespace, declared: !!meta, stored});
})();

export const {config, allows, namespace, declared, stored} = capabilities
