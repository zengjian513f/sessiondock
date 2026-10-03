import {defineStore} from 'pinia'
import {reactive,shallowRef} from 'vue'
export const useRuntimePresentationStore=defineStore('runtime-presentation',()=>{
 const detail=reactive({kind:'',text:'',retry:null as null|(()=>void),attempt:0})
 const progress=reactive({active:false,width:'0%',idle:false,text:''})
 const consoleButton=reactive({icon:'terminal',title:'打开控制台',label:'打开控制台',expanded:undefined as 'true'|'false'|undefined,on:false,unavailable:false})
 const headerLive=reactive({visible:false,tmux:false,frozen:false,attention:'',turn:'',title:'',marker:undefined as string|undefined})
 const backendNotice=shallowRef(''),nodeNotice=shallowRef('')
 const pendingText=shallowRef('')
 const pin=shallowRef<any>(null)
 return {detail,progress,consoleButton,headerLive,backendNotice,nodeNotice,pendingText,pin}
})
