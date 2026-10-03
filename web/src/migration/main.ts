import { createApp } from 'vue'
import { createPinia } from 'pinia'
import WorkspaceShell from '../components/WorkspaceShell.vue'
import * as Settings from './settings'
import * as Shell from './shell'
import SettingsDialog from '../components/SettingsDialog.vue'
import * as Machines from './machines'
// The unchanged JavaScript grid facade is shared with the standalone grid page.
// @ts-expect-error The existing grid implementation has no TypeScript declarations.
import { GridTerm } from '../../../legacy-web/grid/facade.js'

// This IIFE runs as a classic defer script before all scripts that query the
// workspace. Vue owns shell controls; detail, composer and terminal regions
// remain compatibility-owned until their scheduled migration batch. Rust substitutes the hostname in the title.
createApp(WorkspaceShell, {
  hostname: document.title.replace(/ · 会话管理$/, ''),
}).use(createPinia()).mount('#app')

createApp(SettingsDialog).mount('#settings-root')

Object.assign(globalThis, {
  SessionDockShell: Shell,
  SessionDockSettings: Settings,
  SessionDockMachines: Machines,
  GridTerm,
})
