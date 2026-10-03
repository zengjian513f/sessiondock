import {createApp} from 'vue'
import {runtimePinia} from '../stores/runtime/pinia'
import WorkspaceShell from '../components/WorkspaceShell.vue'
import SettingsDialog from '../components/SettingsDialog.vue'
import * as Overlays from './overlays'
import * as Composer from './composer'
import * as SessionUi from './session-ui'
import {createRuntimeApplication} from '../services/runtime/application.js'

Overlays.initialize(runtimePinia)
createApp(WorkspaceShell,{hostname:document.title.replace(/ · 会话管理$/,'')}).use(runtimePinia).mount('#app')
Composer.mount(document.querySelector('#composer-root') as HTMLElement)
createApp(SettingsDialog).use(runtimePinia).mount('#settings-root')
SessionUi.mount()
const application=createRuntimeApplication()
declare global {interface Window {SessionDockRuntime:ReturnType<typeof createRuntimeApplication>}}
window.SessionDockRuntime=application
application.start()
