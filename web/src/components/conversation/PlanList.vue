<script setup lang="ts">
import MessageBubble from './MessageBubble.vue'
import ProcessGroup from './ProcessGroup.vue'
import { operations } from '../../services/conversation/bridge'
import type { ConversationPlan } from '../../services/conversation/bridge'
import type { ConversationStore } from '../../stores/conversation'
const props=defineProps<{plans:ConversationPlan[];store:ConversationStore;revision?:number}>()
function date(value:number) {
  const d=new Date(value), pad=(n:number)=>String(n).padStart(2,'0')
  return `${d.getFullYear()}-${pad(d.getMonth()+1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
}
function lastUser(){for(let i=props.store.state.plans.length-1;i>=0;i--) if(props.store.state.plans[i]?.m?.role==='user') return props.store.state.plans[i]?.m;return null}
</script>
<template>
  <template v-for="p in store.timed(plans)" :key="store.key(p)">
    <div v-if="'time' in p" class="message-time-divider"><time :datetime="new Date(Number(p.time)).toISOString()">{{date(Number(p.time))}}</time></div>
    <div v-else-if="p.gap" class="history-gap">
      <button type="button" class="history-gap-load" :disabled="store.state.gapDisabled" @click="operations().loadHistory(p.gap,$event.currentTarget)">{{store.state.gapLabel || `加载中间 ${Number(p.gap.omitted || 0).toLocaleString()} 条消息`}}</button>
      <div v-if="store.state.gapError" class="history-page-error" role="alert">{{store.state.gapError}}</div>
      <button v-if="store.state.gapError" type="button" class="history-gap-reload" :disabled="store.state.gapReloadBusy" @click="operations().reloadHistory(p.gap,$event.currentTarget)">重新载入当前历史</button>
    </div>
    <ProcessGroup v-else-if="p.g || p.turn" :p="p" :store="store" :revision="(revision || 0)+store.state.revision" />
    <MessageBubble v-else-if="p.m" :m="p.m" :store="store" :revision="(revision || 0)+store.state.revision" :sealed="p.sealedTurnHead" :returned="store.state.returned && p.m===lastUser()" />
  </template>
</template>
