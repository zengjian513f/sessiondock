<script setup lang="ts">
import { computed } from 'vue'
import InnerContent from './InnerContent.vue'
import { outPreviewInfo } from '../../domain/conversation/planning'
import type { ConversationStore } from '../../stores/conversation'
const props = defineProps<{store: ConversationStore; id: string; text: string; path?: string; error?: boolean}>()
const state = props.store.disclosure(props.id)
const info = computed(() => outPreviewInfo(props.text || ''))
const wrapping = computed(() => String(props.text).split('\n').some(line => line.length > 160))
const label = computed(() => `展开全文（另有 ${info.value.omittedLines > 0 ? info.value.omittedLines.toLocaleString() + ' 行' : info.value.omittedChars.toLocaleString() + ' 字符'}）`)
</script>
<template>
  <InnerContent tag="pre" :text="state.full ? text : info.text" diff class="tool-out" :class="{err:error, wrap:state.wrap}" :data-code-path="path || undefined" />
  <div v-if="wrapping || info.text !== text" class="tool-out-actions">
    <button v-if="wrapping" type="button" class="tool-wrap" :class="{on:state.wrap}" :aria-pressed="state.wrap" @click="state.wrap = !state.wrap">{{state.wrap ? '保持原行' : '自动换行'}}</button>
    <button v-if="info.text !== text" class="more" :aria-expanded="state.full" @click="state.full = !state.full">{{state.full ? '收起' : label}}</button>
  </div>
</template>
