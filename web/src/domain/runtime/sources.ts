import { SESSIONDOCK_CLIS } from './cli.js'

export const SOURCES = Object.freeze(Object.fromEntries(
  [...Object.values(SESSIONDOCK_CLIS), {source: 'shell', name: 'SSH', icon: 'i-terminal', color: 'var(--muted)'}]
    .map(cli => [cli.source, {name: cli.name, icon: cli.icon, color: cli.color}]),
))
