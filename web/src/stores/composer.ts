import { shallowReactive } from 'vue'
// Transport and draft objects stay raw in the scoped controller. The store owns
// only the current rendered projection, including the editor's UI state.
export const composerState = shallowReactive<any>({
  visible:false, uid:null, text:'', loading:false, sending:false, addDisabled:false,
  sendDisabled:false, busy:'', placeholder:'输入内容', attachments:[], quotes:[],
  storageError:'', restartable:false, restarting:false, menu:false, dragover:false,
  notice:'', blocked:false, attention:'', question:null, signature:'',
  history:{open:false, items:[], index:-1, state:'ready', error:''},
  questionUi:{selected:[], submitting:null, text:''},
})
let controller:any
export function bindComposerController(value:any) { controller=value }
export function composerOperations():any { return controller }
