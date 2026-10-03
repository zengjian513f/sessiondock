<script setup lang="ts">
import {computed} from 'vue'
import PlanList from './PlanList.vue'
import InnerContent from './InnerContent.vue'
import { operations } from '../../services/conversation/bridge'
import type { ConversationStore } from '../../stores/conversation'
const props=defineProps<{store:ConversationStore;revision:number}>()
const plans=computed(()=>props.store.state.question ? [...props.store.state.plans,{m:props.store.state.question,key:`live-question:${props.store.state.question.id || props.store.state.question.call_id}`}]:props.store.state.plans)
</script>
<template>
  <PlanList :plans="plans" :store="store" :revision="revision" />
  <div v-if="!store.state.question && store.state.activity" id="activity" class="activity" :class="store.state.activity.state" :data-state="store.state.activity.state" role="status" aria-live="polite" :title="store.state.activity.reason || undefined"><i /><span>{{({working:'Working…',waiting:'等待回答',aborted:'已中断',failed:'执行失败'} as Record<string,string>)[store.state.activity.state]}}</span></div>
  <div v-if="store.state.sideThread" class="terminal-thread-notice" role="status"><div class="terminal-thread-copy"><strong>终端当前位于 Codex side thread</strong><span>此会话框仍跟随 main thread，因此不会显示 side 内容。在终端按 Ctrl+/ 可切回 main thread。</span></div><button type="button" @click="operations().revealNativeTerminal(store.state.uid)">查看 side thread</button></div>
  <div v-if="store.state.queued.length" id="queued-sends" class="queued-sends" role="status" aria-live="polite">
    <div v-for="item in store.state.queued" :key="item.request_id" class="msg queued-send" :class="{lost:item.state==='lost'}" data-role="user" :data-request-id="item.request_id" :data-state="item.state || 'queued'" :data-cli-queued="!['lost','interrupted'].includes(item.state) && item.cli_queued_at != null ? '1' : undefined">
      <InnerContent class="mb" :html="operations().md(String(item.text || ''),true,[],{uid:store.state.uid,agent:null})" :syntax="false" />
      <small class="queued-send-state">{{item.state==='lost' ? '未送达，请到终端查看' : item.state==='interrupted' ? 'CLI 已中断，未确认处理，请到终端查看' : item.cli_queued_at != null ? '已进入 CLI 队列，当前步骤结束后处理' : '已发送，等待 CLI 处理'}}<button v-if="['lost','interrupted'].includes(item.state)" type="button" class="queued-send-dismiss" @click="operations().dismissQueuedSend(store.state.uid,item.request_id)">关闭</button></small>
    </div>
  </div>
</template>
