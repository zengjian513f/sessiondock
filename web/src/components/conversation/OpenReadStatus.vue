<script setup lang="ts">
import { runtimePinia } from '../../stores/runtime/pinia'
import { useRuntimePresentationStore } from '../../stores/runtime/presentation'
const status = useRuntimePresentationStore(runtimePinia).detail
</script>
<template><div :class="status.kind">{{status.text}}<template v-if="status.retry">{{' '}}<button type="button" class="btn" @click="status.retry()">重试读取</button><template v-if="status.attempt<=3">{{` 正在自动重试（${status.attempt}/3）…`}}</template></template></div></template>
