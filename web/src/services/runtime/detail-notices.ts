import {h,render,defineComponent,ref} from 'vue'
import OpenReadStatus from '../../components/conversation/OpenReadStatus.vue'
import {SOURCES} from '../../domain/runtime/sources'
import {runtimePinia} from '../../stores/runtime/pinia'
import {useRuntimePresentationStore} from '../../stores/runtime/presentation'
const state=useRuntimePresentationStore(runtimePinia)
let detailHost:DocumentFragment|null=null
export function showDetailState(kind:string,text:string){
 if(detailHost)render(null,detailHost as unknown as HTMLElement)
 Object.assign(state.detail,{kind,text,retry:null,attempt:0})
 detailHost=document.createDocumentFragment()
 render(h(OpenReadStatus),detailHost as unknown as HTMLElement)
 document.querySelector('#detail')!.replaceChildren(detailHost)
}
export function showOpenRetry(retry:()=>void,attempt:number){Object.assign(state.detail,{retry,attempt})}
export const HeaderSource=defineComponent({props:{source:String},setup:p=>()=>{
 const source=p.source!,live=state.headerLive
 return h('span',{class:'ico'},[h('svg',{class:'ico source-icon','data-source':source,'aria-hidden':'true',style:{color:SOURCES[source]!.color}},[h('use',{href:`#${SOURCES[source]!.icon}`})]),h('span',{id:'dlive','data-marker':live.marker,class:{'item-status':true,visible:live.visible,tmux:live.tmux,frozen:live.frozen,'input-attention':!!live.attention,'input-question':live.attention==='question','turn-working':live.turn==='working','turn-waiting':live.turn==='waiting'},title:live.title,'aria-label':live.title},live.frozen?[h('svg',{class:'ui-icon','aria-hidden':'true'},[h('use',{href:'#i-pause'})])]:live.attention==='question'?'?':'')])
}})
export const TimelinePinNotice=defineComponent({props:{clear:{type:Function,required:true}},setup:p=>{const busy=ref(false);return()=>{
 const meta=state.pin, pin=meta?.timeline_pin;if(!pin)return null
 const text=pin.cli?'已同步终端里的回滚，显示到回滚点为止':pin.retired?`固定显示已失效：${pin.retired_message||pin.retired_reason||'原生记录已变化'}。CLI 未回滚。`:'已固定显示到所选输入之前，CLI 未回滚；原生记录继续后自动失效。'
 return h('div',{id:'timeline-pin-notice',role:'status','data-retired':String(!!pin.retired),'data-retired-reason':pin.retired_reason?String(pin.retired_reason):undefined,'data-cli':pin.cli?'true':undefined,style:'display:flex;flex-wrap:wrap;align-items:center;gap:6px 10px;padding:8px 12px;flex:none;border-bottom:1px solid var(--border);font-size:13px'},[h('span',text),...pin.cli?[]:[h('button',{type:'button',class:'btn',id:'timeline-pin-clear',disabled:busy.value,title:'只移除 SessionDock 的显示固定，不会回滚 CLI',onClick:async()=>{busy.value=true;try{await p.clear(meta.uid,null)}finally{busy.value=false}}},pin.retired?'清除记录':'取消固定')]])
}}})
export function createDetailNotices(metadata:{timelinePinEnabled():boolean;pinTimeline(uid:string,target:null):Promise<unknown>}){
 let host:DocumentFragment|undefined
 return {renderTimelinePinNotice(meta:any){if(host)render(null,host as unknown as HTMLElement);host=undefined;const pin=meta?.timeline_pin;state.pin=metadata.timelinePinEnabled()&&meta&&!meta.agent_id&&pin&&typeof pin==='object'&&!(pin.cli&&pin.retired)?meta:null;if(!state.pin)return;host=document.createDocumentFragment();render(h(TimelinePinNotice,{clear:metadata.pinTimeline}),host as unknown as HTMLElement);const heading=document.querySelector('#detail > .dhead');if(heading)heading.after(host);else document.querySelector('#detail')!.prepend(host)}}
}
