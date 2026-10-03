<script setup lang="ts">
import { ref, nextTick } from 'vue'
import SidebarRow from './SidebarRow.vue'
import type { SidebarView, SidebarActions, SidebarGestures, ResourceCell } from '../../domain/sidebar/types'
defineProps<{view: SidebarView; actions: SidebarActions; gestures: SidebarGestures; resources: Map<string, ResourceCell[]>}>()
const input = ref<HTMLInputElement>()
const add = ref<HTMLButtonElement>()
function focusInput() { void nextTick(() => input.value?.focus()) }
function focusAdd() { void nextTick(() => add.value?.focus()) }
</script>
<template>
  <div v-if="view.empty" class="empty" :class="{'search-empty': view.searching}">{{ view.empty }}<button v-if="view.searching" type="button" class="btn" @click="actions.exitSearch()">返回全部会话</button></div>
  <div v-for="group in view.groups" :key="group.key" class="group" :class="{closed: group.closed}" :data-key="group.key" @mousedown="gestures.mousedown" @pointerdown="gestures.pointerdown" @pointerup="gestures.pointerup" @pointercancel="gestures.pointercancel" @pointerleave="gestures.pointerleave" @contextmenu="gestures.contextmenu" @click.capture="gestures.clickCapture" @selectstart="gestures.selectstart">
    <div class="ghead" @click="actions.groupFold(group.key, $event)">
      <input v-if="view.picking" type="checkbox" class="ghead-pick" :checked="group.picked" :indeterminate="group.indeterminate" :aria-label="`选中「${group.label}」下的全部会话`" @click.stop="actions.groupPick(group.pickUids, ($event.currentTarget as HTMLElement).closest('.group') as HTMLElement)">
      <span class="caret">▼</span><span class="gname" :title="group.key"><template v-if="group.node"><span class="node-badge" :data-node-color="group.nodeColor">{{ group.node }}</span>{{ ' ' }}</template>{{ group.path || group.label }}</span>
      <span class="gcount">{{ group.count }}</span><button v-if="view.groupMode" type="button" class="session-group-delete" :aria-label="`删除分组 ${group.label}`" :title="`删除分组 ${group.label}`" :disabled="view.groupBusy" @click.stop="actions.removeGroup(group.label)">×</button>
    </div>
    <div class="glist"><SidebarRow v-for="row in group.rows" :key="row.key" :view="row" :actions="actions" :cells="resources.get(row.key)" /></div>
  </div>
  <div v-if="view.createGroup" id="session-group-create-row">
    <button v-if="!view.groupEditing" id="session-group-add" ref="add" type="button" :disabled="view.groupBusy" @click="actions.editGroup(true); focusInput()">＋ 新建分组</button>
    <form v-else @submit.prevent="input && actions.createGroup(input)"><input id="session-group-name" ref="input" placeholder="分组名称" aria-label="新分组名称" autocomplete="off" :disabled="view.groupBusy" @keydown.esc.stop="actions.editGroup(false); focusAdd()"><button type="submit" :disabled="view.groupBusy">创建</button></form>
  </div>
</template>
