import type {HeaderAction} from '../domain/session-ui/types'
export {STOP_STAGE_TEXT} from '../domain/session-ui/stop-status'
import {h,render,nextTick} from 'vue'
import ConsoleHeading from '../components/session-ui/ConsoleHeading.vue'
import SessionHeader from '../components/session-ui/SessionHeader.vue'
import NewSessionDialog from '../components/session-ui/NewSessionDialog.vue'
import BugReportDialog from '../components/session-ui/BugReportDialog.vue'
import TrashDialog from '../components/session-ui/TrashDialog.vue'
import TransferDialog from '../components/session-ui/TransferDialog.vue'
import TransferTasks from '../components/session-ui/TransferTasks.vue'
import SessionNotices from '../components/session-ui/SessionNotices.vue'
import DeletedReceipt from '../components/session-ui/DeletedReceipt.vue'
import PopupStack from '../components/session-ui/PopupStack.vue'
import {sessionUi,bindApp,bindLaunch,appActions,launchActions} from '../stores/session-ui'
// @ts-expect-error The extracted controllers preserve their JavaScript contracts.
import {createHeaderController} from '../services/session-ui/header.js'
// @ts-expect-error The extracted controllers preserve their JavaScript contracts.
import {createTrashController} from '../services/session-ui/trash.js'
// @ts-expect-error The extracted controllers preserve their JavaScript contracts.
import {createTransferController} from '../services/session-ui/transfer.js'
// @ts-expect-error The extracted controllers preserve their JavaScript contracts.
import {createLaunchController} from '../services/session-ui/launch.js'
export {appAlert,appConfirm,floatStack} from '../services/session-ui/popup'
export {appActions as app,launchActions as launch}
let receiptHost:DocumentFragment|null=null
let headerHost:HTMLElement|null=null,transferHost:HTMLElement|null=null,tasksHost:HTMLElement|null=null
function mountHeader(){if(receiptHost){render(null,receiptHost as unknown as HTMLElement);receiptHost=null}if(headerHost)render(null,headerHost);headerHost=document.createElement('div');headerHost.className='dhead';render(h(SessionHeader),headerHost);return headerHost}
function unmountTransfer(){if(transferHost){render(null,transferHost);transferHost.remove();transferHost=null}}
function unmountTasks(){if(tasksHost){render(null,tasksHost);tasksHost.remove();tasksHost=null}}
async function mountTransfer(){unmountTransfer();transferHost=document.createElement('div');document.body.append(transferHost);render(h(TransferDialog),transferHost);await nextTick();return document.getElementById('clone-group-dialog')!}
async function mountTasks(){unmountTasks();tasksHost=document.createElement('div');document.body.append(tasksHost);render(h(TransferTasks),tasksHost);await nextTick();return document.getElementById('transfer-tasks-dialog')!}
function mountDeleted(props:any){if(headerHost){render(null,headerHost);headerHost=null}sessionUi.header=null;if(receiptHost)render(null,receiptHost as unknown as HTMLElement);receiptHost=document.createDocumentFragment();render(h(DeletedReceipt,props),receiptHost as unknown as HTMLElement);document.getElementById('detail')!.replaceChildren(receiptHost)}
export function mount(){const root=document.getElementById('session-ui-root')!;render(h('div',[h(NewSessionDialog),h(BugReportDialog),h(TrashDialog),h(SessionNotices)]),root);render(h(PopupStack),document.getElementById('popup-root')!)}
export function createAppControllers(headerBridge:any,trashBridge:any,transferBridge:any){
 const header=createHeaderController(Object.defineProperties({mountHeader,mountDeleted},Object.getOwnPropertyDescriptors(headerBridge)),sessionUi)
 const trash=createTrashController(trashBridge,sessionUi)
 const transfer=createTransferController(Object.defineProperties({mountTransfer,unmountTransfer,mountTasks,unmountTasks},Object.getOwnPropertyDescriptors(transferBridge)),sessionUi)
 const controller=Object.defineProperties({initialize(){header.initialize();transfer.initialize()}},{...Object.getOwnPropertyDescriptors(header),...Object.getOwnPropertyDescriptors(trash),...Object.getOwnPropertyDescriptors(transfer)})
 controller.initialize=()=>{header.initialize();transfer.initialize()}
 bindApp(controller as unknown as import('../domain/session-ui/types').AppController);controller.initialize();return controller
}
export function createLaunch(bridge:any){const controller=createLaunchController(bridge,sessionUi);bindLaunch(controller);controller.initialize();return controller}

export function pendingAction(spec:HeaderAction){if(sessionUi.header)Object.assign(sessionUi.header.action,spec)}

export function pendingStage(text:string){const fragment=document.createDocumentFragment();render(h('div',{class:'empty new-session-wait'},text),fragment as unknown as HTMLElement);return fragment.firstChild!}

export function messageCount(total:number){if(sessionUi.header){sessionUi.header.total=total;sessionUi.header.metadata[0]!.text=`${total} 条消息`}}

export function consolePlaceholder(){const fragment=document.createDocumentFragment();render(h(ConsoleHeading),fragment as unknown as HTMLElement);return fragment.firstChild!}
export function restoreViews(){if(sessionUi.header){sessionUi.header.views=appActions().sessionViewRows(sessionUi.header.meta);sessionUi.header.viewsOpen=true}}

export function consoleToast(reason:string){sessionUi.consoleToast=reason}

export function setActionPending(pending:boolean){appActions().setActionPending(pending)}
