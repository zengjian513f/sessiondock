<script setup lang="ts">
import { onMounted, reactive, useTemplateRef } from 'vue'
import { createGridController } from './controller'
const ui = reactive({ machine: '', state: { status: '' }, error: false, size: '', selected: '', sessions: [] as any[], takeover: false, readOnly: false, back: false })
let handlers: any = {}
const term = useTemplateRef('term'), grid = useTemplateRef('grid'), keys = useTemplateRef('keys')
onMounted(() => { handlers = createGridController(ui, { term: term.value, grid: grid.value, keys: keys.value }) })
</script>
<template>
  <main>
    <header><div class="app-mark" aria-hidden="true">▰</div><h1>网格终端</h1><span id="machine">{{ ui.machine }}</span></header>
    <div class="toolbar">
      <select id="session" v-model="ui.selected" :hidden="ui.readOnly" aria-label="会话"><option v-for="row in ui.sessions" :key="row.name" :value="row.name">{{ `${row.name} · ${row.cwd || ''}` }}</option></select>
      <button id="connect" :hidden="ui.readOnly" @click="handlers.connect_click?.()">连接</button>
      <button id="takeover" :hidden="!ui.takeover" @click="handlers.takeover_click?.()">抢占</button>
      <span id="status" :class="ui.error ? 'error' : ''" role="status" aria-live="polite">{{ ui.state.status }}</span>
      <span id="size">{{ ui.size }}</span>
      <button id="copy" @click="handlers.copy_click?.()">复制选区</button>
      <button id="paste" :hidden="ui.readOnly" @click="handlers.paste_click?.()">粘贴</button>
      <button id="bottom" @click="handlers.bottom_click?.()">回到底部</button>
      <button id="back" :hidden="!ui.back" @click="handlers.back_click?.()">← 录制列表</button>
    </div>
    <div id="term" ref="term" tabindex="0" @click="handlers.term_click?.($event)" @wheel="handlers.term_wheel?.($event)">
      <canvas id="grid" ref="grid" @mousedown="handlers.grid_mousedown?.($event)"></canvas>
      <textarea id="keys" ref="keys" autocapitalize="off" autocomplete="off" autocorrect="off" spellcheck="false" aria-label="终端输入" @focus="handlers.keys_focus?.($event)" @blur="handlers.keys_blur?.($event)"></textarea>
    </div>
  </main>
</template>
