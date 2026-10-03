<script setup lang="ts">
import { ref, onMounted, onBeforeUnmount, watch } from 'vue'
import type { RowView, SidebarActions, ResourceCell } from '../../domain/sidebar/types'
const props = defineProps<{view: RowView; actions: SidebarActions; cells?: ResourceCell[]}>()
const node = ref<HTMLElement>()
onMounted(() => { if (node.value) props.actions.rowMounted(node.value, props.view.row) })
watch(() => props.view.row, row => { if (node.value) props.actions.rowMounted(node.value, row) }, {flush: 'sync'})
onBeforeUnmount(() => { if (node.value) props.actions.rowUnmounted(node.value) })
</script>
<template>
  <div ref="node" :class="view.classes" :data-key="view.key" :data-uid="view.row.agent ? undefined : view.row.s.uid" :data-owner="view.row.agent ? view.row.s.uid : undefined" :data-agent="view.row.agent?.id" :data-depth="view.row.depth" :data-tmux-name="view.row.s.pending ? view.row.s.tmuxName : undefined" @click="actions.rowClick(view.row, $event)">
    <span class="nest-lead" :aria-hidden="view.row.kids ? 'false' : 'true'"><i v-for="depth in view.row.depth" :key="depth" class="nest-guide"></i><span class="nest-slot"><button v-if="view.row.kids" type="button" class="nest-caret" :aria-expanded="!view.row.closed" :title="`${view.row.closed ? '展开' : '收起'} ${view.row.kids} 项`" :aria-label="`${view.row.closed ? '展开' : '收起'}「${view.row.s.title}」下的 ${view.row.kids} 项`" @click.stop="actions.nestFold(view.row.s.uid)"></button></span></span>
    <input v-if="view.pickable" type="checkbox" class="item-pick" tabindex="-1" :checked="view.picked" :aria-label="`选中「${view.row.s.title}」`">
    <span class="ico"><svg class="ico source-icon" :data-source="view.row.s.source" aria-hidden="true" :style="{color: view.iconColor}"><use :href="`#${view.icon}`" /></svg><span class="item-status" :class="view.status.classes" :data-marker="view.status.marker" :title="view.status.title" :aria-label="view.status.title"><svg v-if="view.status.frozen" class="ui-icon" aria-hidden="true"><use href="#i-pause" /></svg><template v-else>{{ view.status.text }}</template></span><span v-if="view.row.agent" class="agent-mark" title="子代理" aria-hidden="true" ><svg class="ui-icon" aria-hidden="true"><use href="#i-tree" /></svg></span></span>
    <div class="body">
      <div class="t" :title="view.title" v-html="view.titleHtml"></div>
      <div class="m">{{ view.meta }}</div>
      <div v-if="view.directory" class="cwd" :title="view.row.s.cwd || ''" :data-node-name="view.directory.node"><span v-if="view.directory.node" class="cwd-machine"><span class="node-badge" :data-node-color="view.directory.color">{{ view.directory.node }}</span></span><span class="cwd-path" :data-path="view.directory.path" :data-cwd-color="view.directory.pathColor || ''">{{ (view.directory.label || view.directory.path).slice(0, (view.directory.label || view.directory.path).length - view.directory.leaf.length) }}<span class="cwd-leaf">{{ view.directory.leaf }}</span></span></div>
      <div v-if="view.snippet" class="snip" :title="view.snippet" v-html="view.snippetHtml"></div>
      <div v-if="!view.row.agent" class="session-row-group">{{ view.group }}</div>
    </div>
    <button v-if="cells" type="button" class="item-resources" aria-haspopup="dialog" :aria-label="view.row.agent ? '查看所属会话资源' : '查看会话资源'" :data-resource-signature="JSON.stringify(cells)" @click.stop="actions.resourceOpen(view.row.s)" @mousedown.stop @keydown.stop><span v-for="cell in cells" :key="cell.field" class="item-resource" :data-resource="cell.field" :aria-label="`${cell.label}：${cell.value}`"><svg class="ui-icon" aria-hidden="true"><use :href="`#i-${cell.icon}`" /></svg><span class="item-resource-value">{{ cell.value }}</span></span></button>
    <button v-if="!view.row.agent && !view.row.s.pending" type="button" class="star-toggle item-star" :class="{on: view.row.s.starred}" :data-star-uid="view.row.s.uid" :title="view.row.s.starred ? '取消星标' : '标为星标'" :aria-label="view.row.s.starred ? '取消星标' : '标为星标'" :aria-pressed="!!view.row.s.starred" :disabled="view.starBusy" @click.stop="actions.star(view.row.s.uid)" ><svg class="ui-icon" aria-hidden="true"><use :href="view.row.s.starred ? '#i-star-filled' : '#i-star'" /></svg></button>
  </div>
</template>
