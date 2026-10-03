<script setup lang="ts">
import { onMounted, reactive } from 'vue'
import { openFile } from './service'
const state = reactive({ failed: false, message: '正在打开文件…' })
onMounted(() => { void openFile(state) })
const retry = () => location.reload()
</script>
<template><header :hidden="!state.failed"><h1 id="file-title">{{ state.failed ? '无法打开文件' : '' }}</h1><button id="file-retry" :hidden="!state.failed" @click="retry">刷新</button></header><main id="file-content" role="status">{{ state.message }}</main></template>
