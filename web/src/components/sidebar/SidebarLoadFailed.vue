<script setup lang="ts">
defineProps<{retry: () => void}>()
</script>
<template><div class="empty load-failed"><p>会话列表暂时无法加载</p><button class="btn load-retry" @click="retry()">重试</button></div></template>
