<script setup lang="ts">
import { terminalState as state, terminalOperations as operations } from '../../stores/terminal'
import { terminalKeys } from '../../domain/terminal/shortcuts'
// The mount adapter increments this when publishing a presentation projection.
// Synchronous publication preserves the original measurement/focus ordering.
defineProps<{ revision: number }>()
</script>
<template>
  <div id="termpane" :class="{hidden: !state.visible, 'term-collapsed': state.collapsed, replay: state.replay, 'output-incomplete': !!state.outputNotice, 'term-shift-select': state.shiftSelect, 'ctrl-locked': state.ctrlArmed}" :style="{height: state.height, '--mobile-terminal-top': state.mobileTop}">
    <div class="term-resizer" id="tgrip" title="拖动终端上边界调整高度" @pointerdown="operations().startTermDrag($event)"></div>
    <span id="term-ctrl-lock" role="status">Ctrl（下一键）</span>
    <div id="xterm"></div>
    <div id="term-output-notice" role="status" :hidden="!state.outputNotice">{{state.outputNotice}}</div>
    <div id="term-timeline" class="term-timeline" aria-label="录制回放进度">
      <span id="tl-status" role="status">{{state.timeline.status}}</span>
      <button type="button" id="tl-play" :title="state.timeline.playLabel" :aria-label="state.timeline.playLabel" @click="operations().timelinePlay()">{{state.timeline.playing ? '❚❚' : '▶'}}</button>
      <div class="tl-track">
        <input type="range" id="tl-seek" min="0" max="1000" :value="state.timeline.seek" step="1" aria-label="回放进度" :aria-valuetext="state.timeline.valueText || undefined" :title="state.timeline.title || undefined" @pointerdown="operations().timelinePointerDown()" @input="operations().timelineInput($event)" @change="operations().timelineChange($event)" @pointerup="operations().timelinePointerUp()" @pointercancel="operations().timelinePointerCancel()">
        <div id="tl-ticks" aria-hidden="true" :data-bounds="state.timeline.bounds || undefined"><span v-for="(tick, index) in state.timeline.ticks" :key="index">{{tick}}</span></div>
      </div>
      <span id="tl-time">{{state.timeline.time}}</span>
      <select id="tl-speed" aria-label="倍速" :value="state.timeline.speed" @change="operations().timelineSpeed($event)"><option value="1">1×</option><option value="2">2×</option><option value="4">4×</option><option value="8">8×</option><option value="16">16×</option></select>
      <button type="button" id="tl-live" title="跳到最新并跟随" aria-label="跳到最新并跟随" :hidden="!state.timeline.live" @click="operations().timelineLive()">最新</button>
    </div>
    <div class="term-keys" aria-label="终端快捷键" @click="operations().shortcutClick($event)">
      <button data-term-modifier="ctrl" title="Ctrl（下一键生效）" aria-label="Ctrl，下一键生效" :class="{on: state.ctrlArmed}" :aria-pressed="state.ctrlArmed ? 'true' : 'false'">Ctrl</button>
      <button data-term-modifier="alt" title="Alt（下一键生效）" aria-label="Alt，下一键生效" :class="{on: state.altArmed}" :aria-pressed="state.altArmed ? 'true' : 'false'">Alt</button>
      <button data-term-modifier="shift" title="Shift 选字：开启后拖动选择文字，绕过 CLI 鼠标捕获；再次点击关闭" aria-label="Shift，锁定本地选字" :class="{on: state.shiftSelect}" :aria-pressed="state.shiftSelect ? 'true' : 'false'">Shift</button>
      <button v-for="key in terminalKeys" :key="key.key" :data-term-key="key.key" :title="key.title" :aria-label="key.title">{{key.label}}</button>
    </div>
  </div>
</template>
