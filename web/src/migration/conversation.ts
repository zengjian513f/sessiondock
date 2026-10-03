import {useConversationMediaStore} from '../stores/runtime/conversation-media'
import {runtimePinia} from '../stores/runtime/pinia'
import { h, render } from 'vue'
import ConversationBody from '../components/conversation/ConversationBody.vue'
import QuestionCard from '../components/conversation/QuestionCard.vue'
import ReadFailure from '../components/conversation/ReadFailure.vue'
import QueuedSends from '../components/conversation/QueuedSends.vue'
import { createConversationStore } from '../stores/conversation'
import type { ConversationStore } from '../stores/conversation'
import { configureBridge, operations } from '../services/conversation/bridge'
import type { ConversationBridge, ConversationPlan, ConversationMessage } from '../services/conversation/bridge'
import * as Planning from '../domain/conversation/planning'
import * as Index from '../domain/conversation/index'
import * as Pages from '../domain/conversation/pages'
import * as Requests from '../services/conversation/requests'

interface Scope {store:ConversationStore;box:HTMLElement;host:HTMLElement | DocumentFragment;root:boolean;revision:number;detached?:boolean;pending?:{store:ConversationStore;host:DocumentFragment;revision:number}}
const scopes=new WeakMap<HTMLElement,Scope>()
const stores=new WeakMap<ConversationStore,Scope>()
const mediaStates=useConversationMediaStore(runtimePinia).states
let active:Scope | undefined
let readFailureHost:DocumentFragment | undefined
let queueHost:DocumentFragment | undefined, queueStage:HTMLElement | undefined
export function stageQueue(stage:HTMLElement|null,uid:string,items:any[]) {
  if(queueStage!==stage || !items.length){
    if(queueHost)render(null,queueHost as unknown as HTMLElement)
    queueHost=undefined;queueStage=undefined
  }
  if(!stage || !items.length)return
  if(!queueHost){queueHost=document.createDocumentFragment();queueStage=stage}
  render(h(QueuedSends,{uid,items}),queueHost as unknown as HTMLElement)
  if(queueHost.firstChild)stage.after(queueHost)
}
export function readFailure(detail:HTMLElement,uid:string,agent:string|null,failure:any) {
  if(readFailureHost)render(null,readFailureHost as unknown as HTMLElement)
  readFailureHost=undefined
  if(!failure)return
  const host=document.createDocumentFragment()
  render(h(ReadFailure,{uid,agent,failure}),host as unknown as HTMLElement)
  const node=host.firstElementChild!
  const heading=detail.querySelector(':scope > .dhead')
  if(heading)heading.after(node);else detail.prepend(node)
  readFailureHost=host
}
function publish(scope:Scope) {
  const body=h(ConversationBody,{store:scope.store,revision:++scope.revision,key:scope.store.state.generation})
  render(scope.root ? h('div',{id:'msgs',class:'msgs'},[body]) : body,scope.host as HTMLElement)
  if(scope.host instanceof DocumentFragment && !scope.root && !scope.detached)
    while(scope.host.firstChild)scope.box.appendChild(scope.host.firstChild)
}
function scopeFor(box:HTMLElement) {
  let scope=scopes.get(box)
  if(!scope){scope={store:createConversationStore(),box,host:box,root:false,revision:0};scopes.set(box,scope);stores.set(scope.store,scope)}
  return scope
}
export function configure(bridge:ConversationBridge) {
  configureBridge({...bridge,
    publishStore:(store:ConversationStore)=>{const scope=stores.get(store);if(scope) publish(scope)},
    mediaBusy:(cursor:string)=>mediaStates.get(cursor)?.busy,
    mediaError:(cursor:string)=>mediaStates.get(cursor)?.error,
    toolHandle:(node:HTMLElement,items:()=>ConversationMessage[])=>Object.defineProperty(node,'_toolItems',{configurable:true,get:items}),
    disclosureHandle:(node:HTMLElement,store:ConversationStore,id:string,items:()=>ConversationMessage[],open:()=>void,fold:()=>void,foldAt:()=>void,openAt:()=>void)=>{
      Object.defineProperties(node,{
        _toolItems:{configurable:true,get:()=>node.classList.contains('grp') ? items() : undefined},
        _turnItems:{configurable:true,get:items},
        _userOpened:{configurable:true,get:()=>store.disclosure(id).userOpened},
        _open:{configurable:true,value:open},_fold:{configurable:true,value:fold},
        _openAtAnchor:{configurable:true,value:openAt},_foldAtAnchor:{configurable:true,value:foldAt},
      })
    },
  })
}
export function mount(detail:HTMLElement) {
  if(active) { active.store.state.generation++; render(null,active.host as HTMLElement);disposeStore(active.store);if(active.pending){active.pending.store.state.generation++;render(null,active.pending.host as unknown as HTMLElement);disposeStore(active.pending.store)} }
  const shellHost=document.createDocumentFragment()
  render(h('div',{id:'msgs',class:'msgs'}),shellHost as unknown as HTMLElement)
  const host=document.createDocumentFragment()
  const store=createConversationStore()
  const scope:Scope={store,host,box:shellHost.firstElementChild as HTMLElement,root:false,revision:0}
  detail.appendChild(scope.box)
  scopes.set(scope.box,scope);stores.set(store,scope);active=scope
  Object.defineProperty(scope.box,'_turnSealPending',{configurable:true,get:()=>scope.store.state.sealPending,set:(value:boolean)=>scope.store.state.sealPending=value})
  return scope.box
}
export function mountHeader(detail:HTMLElement,meta:any,total:number) {
  const heading=operations().head(meta,total) as HTMLElement
  detail.appendChild(heading)
  return heading
}
export function prepare(box:HTMLElement,plans:ConversationPlan[],first:boolean,context:{uid:string;agent:string|null}) {
  const scope=scopeFor(box)
  if(first){
    if(scope.pending){scope.pending.store.state.generation++;render(null,scope.pending.host as unknown as HTMLElement);disposeStore(scope.pending.store)}
    const store=createConversationStore()
    store.state.uid=context.uid;store.state.agent=context.agent
    scope.pending={store,host:document.createDocumentFragment(),revision:0}
    stores.set(store,{store,box,host:scope.pending.host,root:false,revision:0,detached:true})
  }
  const pending=scope.pending!
  pending.store.replace([...pending.store.state.plans,...plans])
  render(h(ConversationBody,{store:pending.store,revision:++pending.revision,key:pending.store.state.generation}),pending.host as unknown as HTMLElement)
}
export function publishPrepared(box:HTMLElement) {
  const scope=scopeFor(box),pending=scope.pending
  if(!pending)return
  scope.store.state.generation++
  render(null,scope.host as HTMLElement)
  disposeStore(scope.store);stores.delete(scope.store)
  scope.store=pending.store;scope.host=pending.host;scope.revision=pending.revision;scope.pending=undefined
  stores.set(scope.store,scope)
  while(pending.host.firstChild)box.appendChild(pending.host.firstChild)
}
export function replace(box:HTMLElement,plans:ConversationPlan[],reset=false,context?:{uid:string;agent:string|null}) {
  const scope=scopeFor(box)
  if(context){scope.store.state.uid=context.uid;scope.store.state.agent=context.agent}
  scope.store.replace(plans,reset);publish(scope)
}
export function append(box:HTMLElement,messages:ConversationMessage[],_before:Node|null=null,{openTail=false}={}) {
  const scope=scopeFor(box);scope.store.append(messages,openTail);publish(scope)
  return [...box.children].filter(n=>n.matches('.msg'))
}
export function sealTools(box:HTMLElement) {const scope=scopeFor(box);scope.store.sealTools();publish(scope);return [...box.children].filter(n=>n.matches('.grp'))}
export function seal(box:HTMLElement,entry:any,{defer=true}={}) {
  const scope=scopeFor(box)
  const changed=scope.store.seal(entry,defer,operations().sticking(),operations().live(entry.meta?.uid))
  publish(scope);if(changed)operations().settle(box)
  return changed
}
export function tail(box:HTMLElement,value:any) {const scope=scopeFor(box);Object.assign(scope.store.state,value);scope.store.state.revision++;publish(scope)}
export function refresh(box:HTMLElement) {const scope=scopes.get(box);if(scope)publish(scope)}
export function gapState(value:Record<string,any>) {if(active){Object.assign(active.store.state,value);publish(active)}}
export function mediaState(cursor:string,value:{busy:boolean;error:string}) {mediaStates.set(cursor,value);if(active)publish(active)}
export function updateMedia() {
  if(!active)return
  // Re-plan references to the changed raw message without replacing disclosure
  // keys or the native live checkpoint owned by the cache/request service.
  active.store.state.plans=[...active.store.state.plans]
  active.store.state.revision++
  publish(active)
}
export function mountQuestion(box:HTMLElement,m:any,actions:any) {
  const scope=scopeFor(box);scope.store.state.uid=m.uid
  render(h(QuestionCard,{m,actions,store:scope.store}),box)
}
export function clearQuestion(box:HTMLElement){render(null,box)}
export const planning=Planning
export const index=Index
export const pages=Pages
export const requests=Requests

function disposeStore(store:ConversationStore){store.$dispose();delete runtimePinia.state.value[store.$id]}
