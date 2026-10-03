<script setup lang="ts">
import {computed} from 'vue'
import PlanList from './PlanList.vue'
import QueuedSends from './QueuedSends.vue'
import { operations } from '../../services/conversation/bridge'
import type { ConversationStore } from '../../stores/conversation'
const props=defineProps<{store:ConversationStore;revision:number}>()
const plans=computed(()=>props.store.state.question ? [...props.store.state.plans,{m:props.store.state.question,key:`live-question:${props.store.state.question.id || props.store.state.question.call_id}`}]:props.store.state.plans)
</script>
<template>
  <PlanList :plans="plans" :store="store" :revision="revision" />
  <div v-if="!store.state.question && store.state.activity" id="activity" class="activity" :class="store.state.activity.state" :data-state="store.state.activity.state" role="status" aria-live="polite" :title="store.state.activity.reason || undefined"><i /><span>{{({working:'Working…',waiting:'等待回答',aborted:'已中断',failed:'执行失败'} as Record<string,string>)[store.state.activity.state]}}</span></div>
  <div v-if="store.state.sideThread" class="terminal-thread-notice" role="status"><div class="terminal-thread-copy"><strong>终端当前位于 Codex side thread</strong><span>此会话框仍跟随 main thread，因此不会显示 side 内容。在终端按 Ctrl+/ 可切回 main thread。</span></div><button type="button" @click="operations().revealNativeTerminal(store.state.uid)">查看 side thread</button></div>
  <QueuedSends :uid="store.state.uid" :items="store.state.queued" />
</template>
