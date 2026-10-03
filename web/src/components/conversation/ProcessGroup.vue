<script setup lang="ts">
import { computed, ref, onMounted, onBeforeUnmount } from 'vue'
import PlanList from './PlanList.vue'
import ToolEntry from './ToolEntry.vue'
import InnerContent from './InnerContent.vue'
import { operations } from '../../services/conversation/bridge'
import { turnProcessSummary, messageTimeRange, planMessages, SEARCH_ROLES } from '../../domain/conversation/planning'
import { openDisclosure } from '../../services/conversation/disclosure'
import type { ConversationPlan } from '../../services/conversation/bridge'
import type { ConversationStore } from '../../stores/conversation'
const props=defineProps<{p:ConversationPlan;store:ConversationStore;revision?:number}>()
const root=ref<HTMLElement>(),toolbar=ref<HTMLElement>()
const id=props.store.key(props.p),state=props.store.disclosure(id)
const items=computed<any[]>(()=>props.p.turn?.items || props.p.g || [])
const plan=computed(()=>props.p.turn?.plan || planMessages(items.value))
const summary=computed(()=>turnProcessSummary(items.value))
const times=computed(()=>messageTimeRange(items.value))
const visible=computed(()=>{const calls=items.value.filter(m=>m.role==='tool');return calls.length ? calls : items.value})
const hasError=computed(()=>items.value.some(m=>m.result?.error || (m.role==='tool_result' && m.error)))
const expandTitle=computed(()=>summary.value.paths.length ? `展开过程\n修改文件：${summary.value.paths.join('\n')}` : '展开过程')
let cancelled=false
function open(){openDisclosure(props.store,id,()=>props.p.turn ? plan.value.length : items.value.length)}
function fold(){state.open=false;operations().publishStore(props.store)}
function toggle(){operations().mutateKeepingMessageAnchor(toolbar.value || root.value,()=>{
  if(props.p.turn){state.open ? fold() : open()}
  else {state.userOpened=true;open()}
})}
function close(){state.userOpened=false;fold()}
// Initial working tails render; completed history remains completely lazy.
if(props.p.open && !state.materialized)state.open=true
if(props.p.turn){
  const found=items.value.some(m=>SEARCH_ROLES.has(m.role) && operations().hasTerm(m.text))
  if(found){if(operations().searchCanOpen()) state.open=true;else state.hashit=true}
}
onMounted(()=>{
  if(!root.value) return
  operations().disclosureHandle(root.value,props.store,id,()=>items.value,open,fold,()=>operations().mutateKeepingMessageAnchor(toolbar.value,fold),()=>operations().mutateKeepingMessageAnchor(toolbar.value,open))
  if(state.open && !state.materialized)queueMicrotask(()=>{if(!cancelled)open()})
  if(props.p.turn) operations().searchProcessAsync(items.value,()=>{
    if(cancelled) return
    if(operations().searchCanOpen()) open();else state.hashit=true
  })
})
onBeforeUnmount(()=>cancelled=true)
function conclusion(){let target=root.value?.nextElementSibling;while(target && !target.matches('.msg')) target=target.nextElementSibling;operations().jumpWithinConversation(target)}
</script>
<template>
  <div ref="root" class="msg" :class="[p.turn ? 'turn-process' : 'grp',{folded:!state.open,hashit:state.hashit,'has-error':p.turn && summary.errors}]" :data-role="p.turn ? 'process' : 'toolgroup'" :data-counted="p.turn ? 'false' : undefined" :data-turn-id="p.turn?.key || undefined" :data-interrupted="p.turn?.interrupted ? 'true' : undefined" :data-inferred="p.turn?.inferred ? 'true' : undefined" :data-time-start="times?.start" :data-time-end="times?.end">
    <template v-if="p.turn">
      <div ref="toolbar" class="turn-toolbar">
        <div class="fold-preview turn-preview"><button type="button" class="fold-toggle" :aria-expanded="state.open" :aria-label="state.open ? '收起本轮过程' : '展开本轮过程'" :title="state.open ? '收起过程' : expandTitle" @click="toggle" />
          <span class="peek turn-peek"><b class="turn-label"><svg class="ui-icon" aria-hidden="true"><use href="#i-process" /></svg></b><span class="turn-stats"><span v-for="value in summary.stats" :key="value">{{value}}</span></span></span>
        </div>
        <div class="turn-nav" :hidden="!state.open"><button type="button" class="turn-nav-btn turn-to-start" title="回到本轮过程开头" aria-label="回到本轮过程开头" @click="operations().jumpWithinConversation(root,'start')">↑ 开头</button>
          <button type="button" class="turn-nav-btn turn-to-conclusion" :hidden="p.turn.hasConclusion===false" :title="p.turn.interrupted ? '跳到本轮中断前的末次进展' : '跳到本轮最终结论'" :aria-label="p.turn.interrupted ? '跳到本轮中断前的末次进展' : '跳到本轮最终结论'" @click="conclusion">{{p.turn.interrupted ? '末次进展 ↓' : '结论 ↓'}}</button>
        </div>
      </div>
      <div class="turn-process-body" :hidden="!state.open"><PlanList v-if="state.materialized" :plans="store.identify(plan.slice(0,state.visible))" :store="store" :revision="revision" /></div>
    </template>
    <template v-else>
      <div class="fold-preview group-preview"><button type="button" class="fold-toggle" title="展开工具调用组" aria-label="展开工具调用组" :aria-expanded="state.open" @click="toggle" />
        <span class="peek group-peek"><span class="group-count">🔧 ×{{visible.length}}{{hasError ? ' ⚠' : ''}}</span><span class="group-outline">
          <span v-for="(m,index) in visible.slice(0,3)" :key="store.messageKey(m)"><i class="group-index">{{index+1}}.</i> <InnerContent tag="code" :text="m.summary || m.name || 'tool'" :class="{'tool-command':/^\s*(?:\$|❯)\s+/.test(m.summary || m.name || 'tool')}" /></span>
          <span v-if="visible.length>3" class="group-rest">… 另有 {{visible.length-3}} 项</span>
        </span></span>
      </div>
      <template v-if="state.materialized"><ToolEntry v-for="m in items.slice(0,state.visible)" :key="store.messageKey(m)" :m="m" :store="store" :revision="revision" /></template>
      <button type="button" class="more disclosure" :hidden="!state.open" :title="state.open ? '收起' : undefined" :aria-label="state.open ? '收起' : undefined" :aria-expanded="state.open ? 'true' : undefined" @click="close">{{state.open ? '收起' : ''}}</button>
    </template>
  </div>
</template>
