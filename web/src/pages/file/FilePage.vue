<script setup lang="ts">
import {defineStore} from 'pinia'
import {runtimePinia} from '../../stores/runtime/pinia'
import { onMounted, reactive } from 'vue'
import { openFile } from './service'
const usePageStore=defineStore('page-file',()=>{const state=reactive({ failed: false, message: '正在打开文件…' });return {state}})
const state=usePageStore(runtimePinia).state
onMounted(() => { void openFile(state) })
const retry = () => location.reload()
</script>
<template><header :hidden="!state.failed"><h1 id="file-title">{{ state.failed ? '无法打开文件' : '' }}</h1><button id="file-retry" :hidden="!state.failed" @click="retry">刷新</button></header><main id="file-content" role="status">{{ state.message }}</main></template>
