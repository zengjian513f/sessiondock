import { defineStore } from 'pinia'
import { runtimePinia } from './runtime/pinia'
import { reactive, shallowRef, markRaw } from 'vue'
import type {SessionUiPresentation,AppController,LaunchController} from '../domain/session-ui/types'
// Presentation only. Native maps/catalogs/drafts and protocol identities stay raw
// inside scoped controllers. This store uses the explicit workspace Pinia.
export const useSessionUiStore = defineStore('ui-session', () => {const state = reactive<SessionUiPresentation>({
 header:null,consoleToast:'',frozen:false,freezeUid:'',stop:'',stopUid:'',stopHidden:true,
 trash:[],trashEmpty:'正在读取回收站…',trashSub:'',trashNote:'',trashError:false,trashDisabled:true,trashPending:'',
 transfer:null,tasks:null,bugAttachments:[],bugStorageError:'',bugToast:null,
 bugNodes:[],bugNode:'',bugNodeVisible:false,bugNodeDisabled:false,bugSources:[],bugSource:'codex',
 bugText:'',bugError:'',bugInputDisabled:false,bugAddDisabled:false,bugSubmitDisabled:false,bugSubmitLabel:'发送',
 bugBusyLabel:'',bugAttachOpen:false,bugAttachPadding:'',bugSending:false,
 newNodes:[],newNode:'',newNodeVisible:false,newSources:[],newSource:'claude',newCwd:'',newError:'',newSubmitDisabled:false,newSubmitLabel:'创建',
 cwdRows:[],cwdVisible:true,cwdActive:-1,cwdTitle:'最近使用',cwdStatus:'',models:{}
})
return {state}
})
export const sessionUi = useSessionUiStore(runtimePinia).state
// JavaScript extraction retains the existing controller call signatures. The
// presentation state above has its own explicit interface and never hosts them.
export const appController = shallowRef<AppController|null>(null), launchController = shallowRef<LaunchController|null>(null)
export function bindApp(value: AppController) { appController.value = markRaw(value) }
export function bindLaunch(value: LaunchController) { launchController.value = markRaw(value) }
export function appActions(): AppController { return appController.value! }
export function launchActions(): LaunchController { return launchController.value! }
