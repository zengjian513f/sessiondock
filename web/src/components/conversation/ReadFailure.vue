<script setup lang="ts">
import { operations } from '../../services/conversation/bridge'
defineProps<{uid:string;agent:string|null;failure:any}>()
</script>
<template>
  <div id="migration-read-error" role="alert" style="padding:8px 12px;flex:none;border-bottom:1px solid var(--border);font-size:13px">
    <span>同步已暂停；当前保留的是先前快照，不代表最新历史。{{failure.message}}{{failure.status ? `（HTTP ${failure.status}）` : ''}} </span>
    <button v-if="operations().retryableReadFailure(failure)" type="button" class="btn" :disabled="!!failure.retrying" @click="operations().retryMigrationRead(uid,agent)">{{failure.retrying ? '正在重试…' : '重试读取'}}</button>
  </div>
</template>
