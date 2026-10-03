<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useOverlaysStore } from '../../stores/overlays'
import { probeStates, reasons } from '../../domain/overlays/resources'
import type { Metrics } from '../../domain/overlays/resources'
import type { ResourcesController } from '../../services/overlays/resources'
import ResourceMetrics from './ResourceMetrics.vue'
const { controller } = defineProps<{controller: ResourcesController}>()
const state = useOverlaysStore()
const dialog = ref<HTMLDialogElement>()
const nodes = computed(() => state.resourceData?.nodes || [])
const stamp = computed(() => state.resourceData?.sampled_at ? new Date(state.resourceData.sampled_at * 1000).toLocaleTimeString('zh-CN', {hour12:false}) : '')
const totals = computed(() => (state.resourceData?.totals?.metrics || state.resourceData?.totals) as Metrics | undefined)
onMounted(() => controller.attach(dialog.value!))
</script>
<template>
<dialog ref="dialog" class="session-resources" aria-labelledby="sr-title" @click="event => {if (event.target === dialog) dialog?.close()}" @close="controller.closed">
<div class="sr-top"><div><h2 id="sr-title" tabindex="0" title="按实际执行机器统计已归属的进程，同一进程只计一次。共享 CLI 内的子代理开销无法仅凭进程树精确拆分。">会话资源</h2><p class="sr-subtitle">{{ state.resourceSubtitle }}</p></div><div class="sr-actions"><button type="button" class="sr-refresh" aria-label="刷新资源" @click="controller.refresh(true)">刷新</button><button type="button" class="sr-close" aria-label="关闭资源面板" @click="dialog?.close()">×</button></div></div>
<div class="sr-toolbar"><div class="sr-scopes" role="group" aria-label="资源统计范围">
<button type="button" data-scope="direct" :aria-pressed="state.resourceScope === 'direct'" title="仅统计归属当前会话的进程，包含 SSH 远端命令；不包含已单独归属子会话的进程。" @click="controller.selectScope('direct')">仅当前会话</button>
<button type="button" data-scope="inclusive" :aria-pressed="state.resourceScope === 'inclusive'" title="包含当前会话及已确认关联的子会话进程，无论在本机还是远端。子代理需要有可识别的进程归属。" @click="controller.selectScope('inclusive')">包含子会话</button>
</div><div class="sr-probe-controls"><span class="sr-probe" role="status" :data-state="state.probeState">{{ probeStates[state.probeState] || '未探测' }}</span></div></div><span class="sr-probe-error sr-error" role="status">{{ state.probeError }}</span>
<div class="sr-content" aria-live="polite"><p v-if="state.resourceMessage" class="sr-empty" :class="{'sr-error':state.resourceError}">{{ state.resourceMessage }}</p><template v-else-if="state.resourceData">
<div class="sr-summary-head"><span title="仅合计当前会话相关机器的已知数据；缺失值不代表零。同一进程只计一次。">{{ nodes.length > 1 ? '跨机器合计' : '合计' }}</span><span>{{ stamp ? `采样 ${stamp}` : '实时采样' }}</span></div>
<dl class="sr-metrics sr-totals"><ResourceMetrics :values="totals" /></dl>
<div class="sr-section-heading">执行机器 <span>{{ nodes.length }}</span></div>
<section v-for="(node, index) in nodes" :key="node.node_id || index" class="sr-node"><div class="sr-node-head"><h3>{{ node.node_name || node.node_id }}</h3><span class="sr-status" :class="{'sr-status-ok':node.status === 'ok'}">{{ node.status === 'ok' ? '在线' : reasons[node.status || ''] || '状态未知' }}</span><span v-if="node.diagnostic" class="sr-diagnostic" :class="{'sr-error':node.diagnostic.state === 'failed'}" :data-probe-state="node.diagnostic.state" :data-probe-seconds="Math.max(0, Math.ceil(Number(node.diagnostic.remaining_seconds) || 0))">{{ probeStates[node.diagnostic.state] || '探测状态未知' }}{{ node.diagnostic.error ? ` · ${node.diagnostic.error}` : '' }}</span></div><dl class="sr-metrics"><ResourceMetrics :values="node.metrics" :fallback="node.status" /></dl></section>
<div v-if="!nodes.length" class="sr-empty" title="未采集的数据不代表零占用。">暂无关联进程</div>
</template></div>
</dialog>
</template>
