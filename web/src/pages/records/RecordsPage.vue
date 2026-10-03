<script setup lang="ts">
import {defineStore} from 'pinia'
import {runtimePinia} from '../../stores/runtime/pinia'
import { computed, onMounted, reactive, useTemplateRef } from 'vue'
import { createRecordsController } from './controller'
const params = new URLSearchParams(location.search)
const nodes = (params.get('nodes') || '').split(',').filter(Boolean)
if (!nodes.length) nodes.push(params.get('node') || '')
const embedded = params.get('embedded') === '1' || window.self !== window.top
const usePageStore=defineStore('page-records',()=>{const ui=reactive({ machine: '', state: { id: '', key: '', node: '', status: '', live: false, ended: false, gaps: 0, bytes: 0 }, error: false, size: '', fitOn: false, empty: false, liveOnly: false, records: [] as any[], nodeNames: {} as Record<string, string> });return {ui}})
const ui=usePageStore(runtimePinia).ui
const visible = computed(() => ui.records.filter(row => !ui.liveOnly || row.live))
const selected = (row: any) => row.id === ui.state.id && (row.node || '') === (ui.state.node || '')
const activeDescendant = computed(() => visible.value.find(selected) ? 'rec-' + ui.state.id : undefined)
const sizeText = (bytes: number) => {
  const n = Number(bytes) || 0
  return n >= 1024 * 1024 ? (n / (1024 * 1024)).toFixed(1) + ' MiB' : (n / 1024).toFixed(1) + ' KiB'
}
const metaText = (row: any) => [row.created_ms ? new Date(row.created_ms).toLocaleString() : '', sizeText(row.bytes), `${row.cols || 0}×${row.rows || 0}`].filter(Boolean).join(' · ')
const gridURL = (row: any) => {
  const url = new URL('grid.html', new URL('.', location.href))
  url.searchParams.set('record', row.id)
  if (row.node) url.searchParams.set('node', row.node)
  if (embedded) url.searchParams.set('embedded', '1')
  return url.href
}
let handlers: any = {}
const xterm = useTemplateRef('xterm')
onMounted(() => { handlers = createRecordsController(ui, { xterm: xterm.value }) })
</script>
<template>
  <main>
    <header><div class="app-mark" aria-hidden="true">▰</div><h1>终端录制</h1><span id="machine">{{ ui.machine }}</span></header>
    <div id="panes">
      <aside id="list">
        <div class="toolbar">
          <button id="refresh" @click="handlers.refresh_click?.()">刷新</button>
          <label><input id="live-only" v-model="ui.liveOnly" type="checkbox">只看进行中</label>
        </div>
        <ul id="records" role="listbox" aria-label="录制列表" tabindex="0" :aria-activedescendant="activeDescendant" @keydown="handlers.records_keydown?.($event)"><li v-for="row in visible" :id="'rec-' + row.id" :key="JSON.stringify([row.node || '', row.id])" role="option" :data-key="JSON.stringify([row.node || '', row.id])" :data-id="row.id" :data-node="row.node || ''" :aria-selected="selected(row)" @click="handlers.openRecord?.(row.id, row.node || null)"><div class="heading"><span class="name">{{ row.name || row.id }}</span><span v-if="nodes.length > 1 || nodes[0]" class="badge node">{{ ui.nodeNames[row.node] || (row.node || '').slice(0, 8) }}</span><span :class="'badge ' + (row.live ? 'live' : 'ended')">{{ row.live ? '进行中' : '已结束' }}</span></div><div class="meta">{{ metaText(row) }}</div><div v-if="row.cwd" class="cwd">{{ row.cwd }}</div><a class="grid-link" :href="gridURL(row)" :target="embedded ? '_self' : '_blank'" rel="noopener" @click.stop>网格回放</a></li></ul>
      </aside>
      <section id="viewer">
        <div class="bar">
          <span id="status" :class="ui.error ? 'error' : ''" role="status" aria-live="polite">{{ ui.state.status }}</span>
          <span id="size">{{ ui.size }}</span>
          <button id="fit" :aria-pressed="ui.fitOn" title="按窗口重新折行" @click="handlers.fit_click?.()">适应窗口</button>
          <button id="copy" @click="handlers.copy_click?.()">复制选区</button>
          <button id="bottom" @click="handlers.bottom_click?.()">回到底部</button>
        </div>
        <div id="xterm" ref="xterm" :class="{ fit: ui.fitOn }"></div>
        <div id="empty" :hidden="!ui.empty">选择左侧的录制</div>
      </section>
    </div>
  </main>
</template>
