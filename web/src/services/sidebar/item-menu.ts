import { h, render } from 'vue'
import SidebarItemMenu from '../../components/sidebar/SidebarItemMenu.vue'
import { runtimePinia } from '../../stores/runtime/pinia'
import { useItemMenuStore } from '../../stores/runtime/item-menu'
import type { ItemMenuAction } from '../../stores/runtime/item-menu'
export function createItemMenuPresentation(handlers:{
  action:(action:ItemMenuAction,button:HTMLButtonElement)=>unknown
  groupPointerenter:(event:PointerEvent)=>void
  groupKeydown:(event:KeyboardEvent)=>void
  pointerover:(event:PointerEvent)=>void
}) {
  const {state} = useItemMenuStore(runtimePinia)
  render(h(SidebarItemMenu,handlers),document.querySelector('#item-menu-root')!)
  return {
    state,
    open(uid:string,deleteLabel:string,reasons:Partial<Record<ItemMenuAction,string>>) {
      Object.assign(state,{uid,deleteLabel,reasons,hidden:false})
    },
    close(){state.hidden=true;state.uid=''},
    position(left:number,top:number){state.left=`${left}px`;state.top=`${top}px`},
    setGroupExpanded(expanded:boolean){state.groupExpanded=expanded},
  }
}
