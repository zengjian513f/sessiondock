import {shallowReactive,markRaw,h,render} from 'vue'
import {splitMessage} from '../../domain/session-ui/popup'
export const popups=shallowReactive<any[]>([])
function popup(type:string,message:unknown,buttons:any[]){const {title,body}=splitMessage(message,type==='confirm'?'请确认':'提示');return new Promise(resolve=>{const spec=markRaw({type,message:String(message??''),title,body,buttons,id:`app-popup-${Math.random().toString(36).slice(2)}`,answered:false,resolve});popups.push(spec)})}
export function appAlert(message:unknown){return popup('alert',message,[{label:'知道了',action:'ok',value:undefined,primary:true,className:'btn primary'}])}
export function appConfirm(message:unknown,{ok='确定',cancel='取消',danger=false}={}){return popup('confirm',message,[{label:cancel,action:'cancel',value:false},{label:ok,action:'ok',value:true,primary:true,className:danger?'btn danger':'btn primary'}])}
export function answer(spec:any,value:unknown,dialog:HTMLDialogElement){if(spec.answered)return;spec.answered=true;if(dialog.open)dialog.close();const index=popups.indexOf(spec);if(index>=0)popups.splice(index,1);spec.resolve(value)}
export function floatStack(){const existing=document.getElementById('float-stack');if(existing)return existing;const fragment=document.createDocumentFragment();render(h('div',{id:'float-stack'}),fragment as unknown as HTMLElement);const stack=fragment.firstChild as HTMLElement;document.body.appendChild(fragment);return stack}
