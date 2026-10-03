<script setup lang="ts">
import { computed, onMounted, onBeforeUnmount, ref } from 'vue'
import { operations } from '../../services/conversation/bridge'
import { formatDuration, outputStats, SEARCH_ROLES, TOOL_ROLES, messageTimeRange } from '../../domain/conversation/planning'
import type { ConversationMessage } from '../../services/conversation/bridge'
import type { ConversationStore } from '../../stores/conversation'
import InnerContent from './InnerContent.vue'
import ToolEntry from './ToolEntry.vue'
import MediaGallery from './MediaGallery.vue'
import FileChange from './FileChange.vue'
import QuestionCard from './QuestionCard.vue'
const props=defineProps<{m:ConversationMessage; store:ConversationStore; revision?:number; sealed?:boolean; returned?:boolean}>()
const root=ref<HTMLElement>()
const state=props.store.disclosure(props.store.messageKey(props.m))
const long=computed(()=>String(props.m.text).length>4000)
const kind=computed(()=>props.m.event_kind || 'session')
const times=computed(()=>messageTimeRange([props.m]))
const pin=computed(()=>operations().pinTarget(props.m))
const pinBusy=ref(false)
let cancelled=false
const searchable=SEARCH_ROLES.has(props.m.role) && props.m.role!=='question'
if (searchable) {
  const hit=operations().searchMessage(props.m.text)
  if (hit?.found) {state.full ||= hit.open; state.hashit= !hit.open}
}
onMounted(()=>{
  if (TOOL_ROLES.has(props.m.role) && root.value) operations().toolHandle(root.value,()=>[props.m])
  if (searchable) operations().searchAsync(props.m.text,()=>{
    if(cancelled) return
    const hit=operations().claimSearchOpen();state.hashit=!hit
    if(hit) state.full=true
  })
})
onBeforeUnmount(()=>cancelled=true)
async function pinHere(){pinBusy.value=true;await operations().pinTimeline(props.store.state.uid,pin.value);pinBusy.value=false}
let markdown:{text:string;full:boolean;media:any;uid:string;agent:string|null;html:string}|undefined
const html=computed(()=>{
  void props.revision
  const text=String(props.m.text || ''),media=props.m.media,uid=props.store.state.uid,agent=props.store.state.agent
  if(markdown && markdown.text===text && markdown.full===state.full && markdown.media===media && markdown.uid===uid && markdown.agent===agent)return markdown.html
  markdown={text,full:state.full,media,uid,agent,html:operations().md(text,state.full,media,{uid,agent})}
  return markdown.html
})
</script>
<template>
  <span v-if="m.silent" class="silent-tool-result" hidden :data-role="m.role" :data-counted="m.counted===false ? 'false' : undefined" />
  <div v-else-if="m.role==='event'" class="timeline-event" :class="[kind,m.event_status,{'has-details':m.details}]" data-role="event" data-counted="false">
    <span v-if="kind==='duration'">耗时 {{formatDuration(m.duration_ms)}}</span>
    <template v-else><b>{{kind==='recap' ? '回顾' : kind==='task' ? '任务' : kind==='compact' ? '上下文' : '会话'}}</b><span>{{m.text || ''}}</span>
      <details v-if="m.details" class="event-details" :open="state.details" @toggle="state.details=($event.target as HTMLDetailsElement).open; state.materialized ||= state.details">
        <summary>查看结果 · {{outputStats(m.details).lines.toLocaleString()}} 行</summary>
        <InnerContent v-if="state.materialized" class="event-detail-body" :html="operations().md(m.details,true)" />
      </details>
    </template>
  </div>
  <div v-else-if="m.changes?.length" class="msg file-change-msg" data-role="tool" :data-result="m.result && m.result.counted!==false ? '1' : undefined" :data-counted="m.counted===false ? 'false' : undefined" :data-time-start="times?.start" :data-time-end="times?.end">
    <div class="file-change-list"><FileChange v-for="(change,index) in m.changes" :key="index" :change="change" :store="store" :id="`diff:${store.messageKey(m)}:${index}`" /></div>
  </div>
  <QuestionCard v-else-if="m.role==='question'" :m="m" :store="store" :actions="m.live ? store.state.questionActions : undefined" :class="{'question-live-shadowed':!m.live && m.call_id===store.state.shadow}" :data-time-start="times?.start" :data-time-end="times?.end" />
  <div v-else-if="TOOL_ROLES.has(m.role)" ref="root" class="msg tool-msg" :data-role="m.role" :data-counted="m.counted===false ? 'false' : undefined" :data-time-start="times?.start" :data-time-end="times?.end"><ToolEntry :m="m" :store="store" :revision="revision" /></div>
  <div v-else ref="root" class="msg" :class="{hashit:state.hashit,'native-interrupted':m.interrupted}" :data-role="m.role" :data-counted="m.counted===false ? 'false' : undefined" :data-turn-id="m.turn_id != null ? String(m.turn_id) : undefined" :data-time-start="times?.start" :data-time-end="times?.end">
    <div class="mb" :class="{clip:long && !state.full}"><InnerContent :html="html" fragment /><MediaGallery :items="m.media" :more="m.media_more" /></div>
    <button type="button" class="more disclosure" :hidden="!long" :title="long ? state.full ? '收起' : `展开全文 (${m.text.length.toLocaleString()} 字符)` : undefined" :aria-label="long ? state.full ? '收起' : `展开全文 (${m.text.length.toLocaleString()} 字符)` : undefined" :aria-expanded="long ? state.full : undefined" @click="state.full=!state.full">{{long ? state.full ? '收起' : `展开全文 (${m.text.length.toLocaleString()} 字符)` : ''}}</button>
    <small v-if="m.interrupted" class="native-message-state" :title="m.interrupt_reason || undefined">已中断</small>
    <button v-if="pin" type="button" class="more disclosure timeline-pin-action" :data-pin-target="pin" :disabled="pinBusy" title="固定显示到这条输入之前；只改网页显示，CLI 不会回滚" aria-label="固定显示到这条输入之前；只改网页显示，CLI 不会回滚" style="width:auto;margin:2px 8px 6px auto;padding:2px 8px;font-size:11px" @click="pinHere">回到此处</button>
    <small v-if="returned" class="queued-send-state returned-to-cli">已被 Esc 退回终端输入框，CLI 未处理</small>
  </div>
</template>
