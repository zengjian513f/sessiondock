import {defineStore} from 'pinia'
import {shallowRef} from 'vue'
import type {MachinesState} from '../machines'
// Request records and timers stay in the controller; its published UI snapshot
// belongs to this Pinia store and is shared by the settings panes.
export const useMachinesProjectionStore=defineStore('ui-machines',()=>{
 const current=shallowRef<MachinesState>()
 return {current}
})
