import { createApp } from 'vue'
import { createPinia } from 'pinia'
import WorkspaceShell from '../components/WorkspaceShell.vue'
import * as Settings from './settings'
import * as Shell from './shell'
import * as Composer from './composer'
import SettingsDialog from '../components/SettingsDialog.vue'
import * as Machines from './machines'
import * as Search from './search'
import * as Sidebar from './sidebar'
import * as Conversation from './conversation'
import * as Terminal from './terminal'
// The unchanged JavaScript grid facade is shared with the standalone grid page.
// @ts-expect-error The existing grid implementation has no TypeScript declarations.
import { GridTerm } from '../../../legacy-web/grid/facade.js'

// This IIFE runs as a classic defer script before all scripts that query the
// workspace. Vue owns shell and terminal controls; terminal protocols and engines
// live in the scoped controller. Rust substitutes the hostname in the title.
createApp(WorkspaceShell, {
  hostname: document.title.replace(/ · 会话管理$/, ''),
}).use(createPinia()).mount('#app')

Composer.mount(document.querySelector('#composer-root') as HTMLElement)

createApp(SettingsDialog).mount('#settings-root')

Object.assign(globalThis, {
  SessionDockComposer: Composer,
  SessionDockShell: Shell,
  SessionDockSettings: Settings,
  SessionDockMachines: Machines,
  SessionDockSearch: Search,
  SessionDockSidebar: Sidebar,
  SessionDockConversation: Conversation,
  SessionDockTerminal: Terminal,
  GridTerm,
})
