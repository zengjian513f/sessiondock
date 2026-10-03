import { createApp } from 'vue'
import { createPinia } from 'pinia'
import WorkspaceShell from '../components/WorkspaceShell.vue'
import * as Settings from '../legacy/settings'
import * as Machines from './machines'
// The unchanged JavaScript grid facade is shared with the standalone grid page.
// @ts-expect-error The existing grid implementation has no TypeScript declarations.
import { GridTerm } from '../../../legacy-web/grid/facade.js'

// This IIFE runs as a classic defer script before all scripts that query the
// workspace. Vue renders directly into #app and never updates the legacy-owned
// regions after this initial mount. Rust substitutes the hostname in the title.
createApp(WorkspaceShell, {
  hostname: document.title.replace(/ · 会话管理$/, ''),
}).use(createPinia()).mount('#app')

Object.assign(globalThis, {
  SessionDockSettings: Settings,
  SessionDockMachines: Machines,
  GridTerm,
})
