import {defineStore} from 'pinia'
import {shallowReactive} from 'vue'
export const useConversationMediaStore=defineStore('ui-conversation-media',()=>({states:shallowReactive(new Map<string,{busy:boolean;error:string}>())}))
