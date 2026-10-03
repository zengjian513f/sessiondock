<script setup lang="ts">
import { computed } from 'vue'
import InnerContent from './InnerContent.vue'
import ClippedOutput from './ClippedOutput.vue'
import MediaGallery from './MediaGallery.vue'
import { outputStats, formatDuration, toolOutputPath } from '../../domain/conversation/planning'
import type { ConversationMessage } from '../../services/conversation/bridge'
import type { ConversationStore } from '../../stores/conversation'
const props = defineProps<{m: ConversationMessage; store: ConversationStore; revision?:number}>()
const id = props.store.messageKey(props.m)
const state = props.store.disclosure(`args:${id}`, !props.m.summary)
function args(open:boolean){state.open=open;state.materialized ||= open}
const result = computed(() => props.m.result)
const status = computed(() => {
  const r = result.value; if (!r) return ''
  const stats = outputStats(r.text || '')
  return [r.error ? '✗ 出错' : '✓ 完成', Number.isInteger(r.exit_code) ? `exit ${r.exit_code}` : '',
    Number.isFinite(+r.duration_s) ? formatDuration(+r.duration_s * 1000) : '',
    stats.lines > 1 ? `${stats.lines.toLocaleString()} 行` : `${stats.chars.toLocaleString()} 字符`].filter(Boolean).join(' · ')
})
</script>
<template>
  <div class="tool-entry" :class="{'args-open':m.role === 'tool' && state.open}" :data-role="m.role" :data-counted="m.counted === false ? 'false' : undefined" :data-result="result && result.counted !== false ? '1' : undefined">
    <template v-if="m.role === 'tool'">
      <div class="tool-head">
        <InnerContent tag="code" :text="m.summary || m.name || 'tool'" :class="{'tool-command':/^\s*(?:\$|❯)\s+/.test(m.summary || '')}" />
        <span v-if="m.name && m.summary" class="tool-meta">{{m.name}}</span>
        <button type="button" class="tool-toggle" :aria-expanded="state.open" :title="state.open ? '收起原始参数' : '展开原始参数'" :aria-label="state.open ? '收起原始参数' : '展开原始参数'" @click="args(!state.open)" />
      </div>
      <div v-if="m.changes_unavailable_reason" class="tool-change-warning tool-meta">未生成文件差异：{{m.changes_unavailable_reason}}；可展开原始参数查看。</div>
      <pre class="tool-args" :hidden="!state.open">{{state.open || state.materialized ? m.text : ''}}</pre>
      <button type="button" class="tool-args-close" :hidden="!state.open" @click="args(false)">收起参数</button>
      <MediaGallery :items="m.media" :more="m.media_more" />
      <template v-if="result">
        <div class="tool-status" :class="{err:result.error}">{{status}}</div>
        <ClippedOutput v-if="String(result.text || '').trim()" :store="store" :id="`result:${id}`" :text="result.text" :error="result.error" :path="toolOutputPath(m)" />
        <MediaGallery :items="result.media" :more="result.media_more" />
      </template>
    </template>
    <template v-else>
      <ClippedOutput :store="store" :id="`output:${id}`" :text="m.name ? `${m.name}\n${m.text}` : m.text" :error="m.error" :path="toolOutputPath(m)" />
      <MediaGallery :items="m.media" :more="m.media_more" />
    </template>
  </div>
</template>
