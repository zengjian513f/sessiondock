<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, watch } from 'vue'
import type { MachinesState, MachinesStore } from '../stores/machines'
import { MACHINE_COLORS } from '../stores/machines'
import type { MachineTarget } from '../services/machines'
const props = defineProps<{state: MachinesState; controller: MachinesStore}>()
const matrix = computed(() => props.controller.matrix(props.state))
const colorLabel = (target: MachineTarget) => MACHINE_COLORS.find(([v]) => v === target.color)?.[1] || '默认'
const focusColor = (id: string) => {
  const button = [...document.querySelectorAll<HTMLButtonElement>('[data-machine-color]')].find(b => b.dataset.machineColor === id)
  button?.focus()
}
const focusGrip = (id: string) => {
  const button = [...document.querySelectorAll<HTMLButtonElement>('[data-machine-grip]')].find(b => b.dataset.machineGrip === id)
  button?.focus()
}
async function palette(target: MachineTarget) {
  const wasOpen = props.state.palette === target.id
  props.controller.palette(wasOpen ? null : target.id)
  if (!wasOpen) {
    await nextTick()
    const wrap = [...document.querySelectorAll<HTMLElement>('.machine-row')].find(r => r.dataset.machine === target.id)
    wrap?.querySelector<HTMLButtonElement>('[aria-selected="true"]')?.focus()
  }
}
async function color(target: MachineTarget, value: string) {
  props.controller.palette(null)
  await props.controller.save(target, {color: value}, `color:${target.id}`)
  await nextTick(); focusColor(target.id)
}
async function saveName(target: MachineTarget, event: Event) {
  const input = event.target as HTMLInputElement
  await props.controller.save(target, {name: input.value}, `name:${target.id}`)
  input.value = props.state.targets.find(t => t.id === target.id)?.name || target.name
}
async function enabled(target: MachineTarget, event: Event) {
  const input = event.target as HTMLInputElement
  await props.controller.save(target, {enabled: input.checked}, `enabled:${target.id}`)
  input.checked = props.state.targets.find(t => t.id === target.id)?.enabled !== false
}
async function renderer(target: MachineTarget, event: Event) {
  const select = event.target as HTMLSelectElement
  await props.controller.chooseRenderer(target, select.value)
  select.value = props.state.targets.find(t => t.id === target.id)?.renderer || target.renderer
}
function rendererTitle(target: MachineTarget) {
  return target.enabled === false ? '已停用：不显示、不检查，视同不存在；勾选后重新接入'
    : target.online === false ? props.controller.bridge.offlineReason(target)
    : '这台机器上会话的控制台怎么画：服务端网格由宿主解析终端、浏览器只画格子；'
      + 'xterm.js 由浏览器自己解析。部署前启动的旧宿主只能用 xterm.js，会自动回落。重新打开控制台后生效。'
}
async function keyboard(target: MachineTarget, event: KeyboardEvent) {
  const step = event.key === 'ArrowUp' ? -1 : event.key === 'ArrowDown' ? 1 : 0
  if (!step) return
  event.preventDefault(); props.controller.move(target.id, step)
  await nextTick(); focusGrip(target.id)
}
let stopDrag: (() => void) | undefined
function startDrag(target: MachineTarget, event: PointerEvent) {
  if (event.button !== 0 || props.state.dragging) return
  event.preventDefault()
  const before = props.state.targets.map(t => t.id)
  props.controller.startDrag(target.id)
  const move = (e: PointerEvent) => {
    if (e.pointerId !== event.pointerId) return
    for (const other of document.querySelectorAll<HTMLElement>('#machine-rows > .machine-row')) {
      if (other.dataset.machine === target.id) continue
      const rect = other.getBoundingClientRect()
      if (e.clientY < rect.top || e.clientY > rect.bottom) continue
      props.controller.dragOver(target.id, other.dataset.machine!, e.clientY > rect.top + rect.height / 2)
      break
    }
  }
  const remove = () => {
    document.removeEventListener('pointermove', move)
    document.removeEventListener('pointerup', finish)
    document.removeEventListener('pointercancel', finish)
    stopDrag = undefined
  }
  const finish = (e: PointerEvent) => {
    if (e.pointerId !== event.pointerId) return
    remove(); props.controller.finishDrag(before)
  }
  stopDrag = remove
  document.addEventListener('pointermove', move)
  document.addEventListener('pointerup', finish)
  document.addEventListener('pointercancel', finish)
}
function outside(event: PointerEvent) {
  if (props.state.palette === null) return
  const wrap = (event.target as Element).closest('.machine-color')
  if (wrap?.closest<HTMLElement>('.machine-row')?.dataset.machine !== props.state.palette) props.controller.palette(null)
}
function escape(event: KeyboardEvent) {
  if (event.key !== 'Escape' || props.state.palette === null) return
  const id = props.state.palette
  props.controller.palette(null); focusColor(id)
  event.preventDefault(); event.stopPropagation()
}
watch(() => props.state.rowRevision, async () => {
  const id = (document.activeElement as HTMLElement | null)?.dataset.machineGrip
  await nextTick(); if (id) focusGrip(id)
})
onMounted(() => {
  document.addEventListener('pointerdown', outside)
  document.querySelector('#settings-dialog')?.addEventListener('keydown', escape as EventListener)
})
onUnmounted(() => {
  stopDrag?.()
  document.removeEventListener('pointerdown', outside)
  document.querySelector('#settings-dialog')?.removeEventListener('keydown', escape as EventListener)
})
</script>

<template>
  <p class="setting-group-title">机器<small>保存在中央服务端，所有浏览器一致。接入和移除机器仍在服务器上操作</small></p>
  <div id="machine-rows">
    <p v-if="!state.targets.length" class="setting-note">暂无可用机器。</p>
    <div v-for="target in state.targets" :key="`${target.id}:${state.rowRevision}`" class="machine-row"
      :class="{'machine-off': target.enabled === false, dragging: state.dragging === target.id}" :data-machine="target.id">
      <div class="machine-head">
        <template v-if="!target.local">
          <button type="button" class="machine-grip" :data-machine-grip="target.id" title="拖动调整顺序；也可按 ↑ ↓"
            :aria-label="`调整 ${target.name} 的顺序：拖动，或按上下方向键`"
            @pointerdown="startDrag(target, $event)" @keydown="keyboard(target, $event)"><span aria-hidden="true">⋮⋮</span></button>
          <input type="checkbox" class="machine-enabled" :checked="(state.controls.get(`enabled:${target.id}`) as boolean | undefined) ?? (target.enabled !== false)" :disabled="state.busy.has(`enabled:${target.id}`)"
            :title="target.enabled !== false ? '取消勾选后这台机器不显示、不检查' : '勾选后重新接入这台机器'"
            :aria-label="`启用 ${target.name}`" @change="enabled(target, $event)">
        </template>
        <div class="machine-color">
          <span v-if="target.local" class="machine-swatch" :data-node-color="target.color"></span>
          <template v-else>
            <button type="button" class="machine-swatch" :data-node-color="target.color" :data-machine-color="target.id"
              aria-haspopup="true" :aria-expanded="state.palette === target.id ? 'true' : 'false'"
              :aria-label="`${target.name} 的配色：${colorLabel(target)}`" :disabled="state.busy.has(`color:${target.id}`)" @click="palette(target)"></button>
            <div class="machine-palette" role="listbox" :aria-label="`${target.name} 的配色`" :hidden="state.palette !== target.id">
              <button v-for="[value, name] in MACHINE_COLORS" :key="value" type="button" class="machine-swatch" :data-node-color="value"
                role="option" :aria-selected="value === target.color ? 'true' : 'false'" :aria-label="name" :title="name" @click="color(target, value!)"></button>
            </div>
          </template>
        </div>
        <b v-if="target.local">{{ target.name }}</b>
        <input v-else type="text" :value="state.controls.get(`name:${target.id}`) ?? target.name" maxlength="80" :aria-label="`${target.name} 的名称`"
          :disabled="state.busy.has(`name:${target.id}`)" @focus="controller.editing(true)" @blur="controller.editing(false)"
          @input="controller.draft(`name:${target.id}`, ($event.target as HTMLInputElement).value)" @change="saveName(target, $event)" @keydown.enter.prevent="($event.target as HTMLInputElement).blur()">
      </div>
      <div class="machine-fields">
        <select class="machine-renderer" :aria-label="`${target.name} 的控制台渲染`" :value="state.controls.get(`renderer:${target.id}`) ?? (target.renderer || 'grid')"
          :disabled="target.enabled === false || state.busy.has(`renderer:${target.id}`)" :title="rendererTitle(target)" @change="renderer(target, $event)">
          <option v-for="[value, label] in controller.bridge.rendererOptions" :key="value" :value="value">{{ label }}</option>
        </select>
      </div>
    </div>
  </div>
  <p class="setting-group-title">AI 客户端</p>
  <div id="client-matrix">
    <table v-if="matrix.rows.length" class="client-matrix">
      <thead><tr><th scope="col">机器</th><th v-for="source in matrix.sources" :key="source" scope="col">{{ controller.bridge.sourceNames[source]?.name || source }}</th></tr></thead>
      <tbody><tr v-for="row in matrix.rows" :key="row.target.id" :data-machine="row.target.id">
        <th scope="row"><span class="machine-swatch" :data-node-color="row.target.color"></span>{{ row.target.name }}</th>
        <td v-if="row.status" :colspan="Math.max(1, matrix.sources.length)" class="client-status" :title="row.error || undefined">{{ row.status }}</td>
        <template v-else><td v-for="cell in row.cells" :key="cell.source" :data-client-source="cell.source"
          :class="{'client-missing': !cell.client}" :data-state="cell.state" :title="cell.title">
          <span v-if="cell.client" class="client-cell"><code class="client-version">{{ cell.client.version || '?' }}</code><button
            type="button" class="client-update" :disabled="cell.running || state.busy.has(`client:${row.target.id}:${cell.client.id}`)"
            :aria-label="`更新 ${row.target.name} 上的 ${controller.bridge.sourceNames[cell.client.source]?.name || cell.client.source}`"
            @click="controller.updateClient(row.target, cell.client)">{{ cell.running || state.busy.has(`client:${row.target.id}:${cell.client.id}`) ? '…' : '↑' }}</button></span>
          <template v-else>无</template>
        </td></template>
      </tr></tbody>
    </table>
  </div>
  <p id="machine-note" class="setting-note" :hidden="!state.note" role="status" aria-live="polite" :data-state="state.error ? 'error' : 'ok'">{{ state.note }}</p>
</template>
