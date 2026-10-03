<script setup lang="ts">
defineProps<{active: number; total: number; known: boolean; activeOnly: boolean; select: (active: boolean) => void; keydown: (event: KeyboardEvent) => void}>()
</script>
<template>
  <button type="button" id="livecount" role="radio" :aria-checked="activeOnly" :tabindex="activeOnly ? 0 : -1" :class="{on: activeOnly}" :data-unknown="!known ? 'true' : undefined" :title="known ? '只显示活跃会话' : '运行状态未知：Rust 后端尚未实现进程探测'" :aria-label="known ? `${active} 个活跃会话` : '运行状态未知，尚未实现进程探测'" @click="select(true)" @keydown="keydown"><b id="session-active">{{ known ? active : '?' }}</b></button>
  <button type="button" id="allcount" role="radio" :aria-checked="!activeOnly" :tabindex="activeOnly ? -1 : 0" :class="{on: !activeOnly}" title="显示全部会话" :aria-label="`${total} 个总会话`" @click="select(false)" @keydown="keydown"><b id="session-total">{{ total }}</b></button>
</template>
