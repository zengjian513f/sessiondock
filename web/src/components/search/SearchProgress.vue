<script setup lang="ts">
import { computed } from 'vue'
import { state } from '../../stores/search'
import { exitSearch } from '../../services/search/runtime'
import { nodeStatus } from '../../domain/search/progress'
const known = computed(() => state.progress.totalKnown && state.progress.total !== null && state.progress.total > 0)
const percent = computed(() => known.value ? Math.min(100, Math.floor(state.progress.done / state.progress.total! * 100)) : 0)
const label = computed(() => known.value ? `${state.progress.done} / ${state.progress.total} 个会话 · ${percent.value}%` : state.progress.done ? `已扫描 ${state.progress.done} 个会话` : state.progress.totalKnown ? '已扫描 0 个会话' : '正在读取会话数量…')
</script>
<template>
  <div id="search-progress" role="status" aria-live="polite" :class="{on: state.progress.active}">
    <div class="search-progress-head"><span>全文搜索</span><b>{{ label }}</b><button type="button" class="btn" id="search-cancel" @click="exitSearch">取消</button></div>
    <div class="search-progress-track" role="progressbar" aria-label="会话扫描进度" aria-valuemin="0" aria-valuemax="100" :hidden="!known" :aria-valuenow="known ? String(percent) : undefined" :aria-valuetext="label"><i :style="{width: state.progress.active && known ? `${Math.min(100, state.progress.done / state.progress.total! * 100)}%` : '0%'}"></i></div>
    <div class="search-progress-nodes"><div v-for="node in state.progress.nodes" :key="node.id" class="search-progress-node" :data-state="node.state"><span>{{ node.name }}</span><span>{{ nodeStatus(node) }}</span></div></div>
  </div>
</template>
