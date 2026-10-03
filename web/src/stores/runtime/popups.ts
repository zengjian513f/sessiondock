import {defineStore} from 'pinia'
import {shallowReactive} from 'vue'
export const usePopupStore=defineStore('ui-popups',()=>({popups:shallowReactive<any[]>([])}))
