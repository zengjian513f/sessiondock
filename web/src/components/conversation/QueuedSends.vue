<script setup lang="ts">
import InnerContent from './InnerContent.vue'
import { operations } from '../../services/conversation/bridge'
defineProps<{uid:string;items:any[]}>()
</script>
<template>
  <div v-if="items.length" id="queued-sends" class="queued-sends" role="status" aria-live="polite">
    <div v-for="item in items" :key="item.request_id" class="msg queued-send" :class="{lost:item.state==='lost'}" data-role="user" :data-request-id="item.request_id" :data-state="item.state || 'queued'" :data-cli-queued="!['lost','interrupted'].includes(item.state) && item.cli_queued_at != null ? '1' : undefined">
      <InnerContent class="mb" :html="operations().md(String(item.text || ''),true,[],{uid,agent:null})" :syntax="false" />
      <small class="queued-send-state">{{item.state==='lost' ? '未送达，请到终端查看' : item.state==='interrupted' ? 'CLI 已中断，未确认处理，请到终端查看' : item.cli_queued_at != null ? '已进入 CLI 队列，当前步骤结束后处理' : '已发送，等待 CLI 处理'}}<button v-if="['lost','interrupted'].includes(item.state)" type="button" class="queued-send-dismiss" @click="operations().dismissQueuedSend(uid,item.request_id)">关闭</button></small>
    </div>
  </div>
</template>
