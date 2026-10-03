<script setup lang="ts">
import { runtimePinia } from '../../stores/runtime/pinia'
import { useItemMenuStore } from '../../stores/runtime/item-menu'
import type { ItemMenuAction } from '../../stores/runtime/item-menu'
const props = defineProps<{
  action:(action:ItemMenuAction,button:HTMLButtonElement)=>unknown
  groupPointerenter:(event:PointerEvent)=>void
  groupKeydown:(event:KeyboardEvent)=>void
  pointerover:(event:PointerEvent)=>void
}>()
const menu = useItemMenuStore(runtimePinia).state
const actions: {id:ItemMenuAction,label:string}[] = [
  {id:'copy-identity',label:'复制会话标识'}, {id:'stop',label:'停止会话'},
  {id:'hide',label:'隐藏父会话'}, {id:'detach',label:'解除附属'},
  {id:'attach',label:'附属到…'}, {id:'delete',label:'删除会话'},
  {id:'clone',label:'移动 / 复制整组…'}, {id:'group',label:'分组 '}, {id:'pick',label:'多选…'},
]
function click(action:ItemMenuAction,event:MouseEvent) {
  if (menu.reasons[action]) return
  return props.action(action,event.currentTarget as HTMLButtonElement)
}
</script>
<template>
  <div id="item-menu" class="ctx-menu" :hidden="menu.hidden" role="menu" aria-label="会话操作" :style="{left:menu.left,top:menu.top}" @pointerover="pointerover">
    <button v-for="item in actions" :key="item.id" type="button" role="menuitem" :data-act="item.id"
      :title="menu.reasons[item.id] || undefined" :data-unavailable-reason="menu.reasons[item.id] || undefined"
      :aria-disabled="menu.reasons[item.id] ? 'true' : undefined" :aria-description="menu.reasons[item.id] || undefined"
      :aria-controls="item.id==='group' ? 'session-group-menu' : undefined" :aria-haspopup="item.id==='group' ? 'menu' : undefined"
      :aria-expanded="item.id==='group' ? menu.groupExpanded : undefined"
      @click="click(item.id,$event)" @pointerenter="item.id==='group' && groupPointerenter($event)" @keydown="item.id==='group' && groupKeydown($event)">
      {{item.id==='delete' ? menu.deleteLabel : item.label}}<span v-if="item.id==='group'" aria-hidden="true">›</span>
    </button>
  </div>
</template>
