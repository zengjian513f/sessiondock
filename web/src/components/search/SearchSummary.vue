<script setup lang="ts">
import { state } from '../../stores/search'
import { exitSearch } from '../../services/search/runtime'
</script>
<template>
  <div id="side-search-state" :hidden="!state.summary.active">
    <div class="side-search-summary"><strong>⌕ <span id="side-search-label">{{ state.summary.full ? '搜索结果' : '筛选结果' }}</span></strong><span id="side-search-query">{{ state.summary.query }}</span><span id="side-search-count">{{ state.summary.count }} 条</span></div>
    <button type="button" class="btn" id="side-search-exit" @click="exitSearch">退出搜索 ×</button>
  </div>
</template>
