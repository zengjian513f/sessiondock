import {defineComponent,h,render} from 'vue'
import SidebarGroupMenu from '../../components/sidebar/SidebarGroupMenu.vue'
import {runtimePinia} from '../../stores/runtime/pinia'
import {useGroupsStore} from '../../stores/runtime/groups'
export function mountGroupMenu(assign:(name:string)=>void,keydown:(event:KeyboardEvent)=>void){
 const state=useGroupsStore(runtimePinia)
 const Menu=defineComponent({setup:()=>()=>h('div',{id:'session-group-menu',class:'ctx-menu',hidden:state.menuHidden,role:'menu','aria-label':'会话分组',style:{left:state.menuLeft,top:state.menuTop}},[h(SidebarGroupMenu,{items:state.menuItems,busy:state.busy,assign,keydown})])})
 const fragment=document.createDocumentFragment();render(h(Menu),fragment as unknown as HTMLElement);document.getElementById('session-group-menu')!.replaceWith(fragment)
 return document.getElementById('session-group-menu')!
}
