<script setup lang="ts">
import { onMounted, onUnmounted } from 'vue'
import type { SettingsState, SettingsBridge } from '../legacy/settings-bridge'
const { state, bridge } = defineProps<{ state: SettingsState; bridge: SettingsBridge }>()
const value = (event: Event) => (event.target as HTMLInputElement).value
let scaleSliderActive = false
function startScale(event: PointerEvent) {
  scaleSliderActive = true
  bridge.scaleIndicator(Number(value(event)), true)
}
function inputScale(event: Event) {
  bridge.scale(value(event), true)
  bridge.scaleIndicator(Number(value(event)), scaleSliderActive)
}
function finishScale() {
  if (!scaleSliderActive) return
  scaleSliderActive = false
  bridge.scaleIndicator(bridge.read().scale)
}
function resetScale() {
  bridge.scale(100, true)
  bridge.scaleIndicator(100)
}
onMounted(() => {
  window.addEventListener('pointerup', finishScale)
  window.addEventListener('pointercancel', finishScale)
})
onUnmounted(() => {
  window.removeEventListener('pointerup', finishScale)
  window.removeEventListener('pointercancel', finishScale)
})
</script>

<template>
    <div class="setting-row">
      <label for="setting-scale"><b>界面缩放</b><small>拖动或双指调整，所有布局生效</small></label>
      <div class="setting-scale-control">
        <input id="setting-scale" type="range" min="50" max="150" step="1" :value="state.scale" @pointerdown="startScale" @input="inputScale" @blur="finishScale">
        <div><output id="setting-scale-value" for="setting-scale">{{ state.scale }}%</output>
          <button id="setting-scale-reset" class="btn" type="button" @click="resetScale">重置</button></div>
      </div>
    </div>
    <label class="setting-row" for="setting-font">
      <span><b>等宽字体</b><small>终端与工具输出共同使用</small></span>
      <select id="setting-font" :value="state.font" @change="bridge.font(value($event))">
        <option value="ubuntu">Ubuntu Sans Mono（26.04 默认）</option>
        <option value="cascadia">Cascadia Mono（内置）</option>
        <option value="system">系统等宽字体</option>
        <option value="consolas">Consolas / Consola</option>
      </select>
    </label>
    <label class="setting-row" for="setting-theme">
      <span><b>颜色方案</b><small>可跟随操作系统自动切换</small></span>
      <select id="setting-theme" :value="state.theme" @change="bridge.theme(value($event))">
        <option value="system">跟随系统</option>
        <option value="light">浅色</option>
        <option value="dark">深色</option>
      </select>
    </label>
    <div class="setting-row">
      <span><b>桌面应用</b><small>安装后以独立窗口运行，不显示浏览器地址栏</small></span>
      <button type="button" class="btn" data-pwa-install-button :disabled="state.pwa.disabled" :title="state.pwa.title" @click="bridge.pwa.install">{{ state.pwa.text }}</button>
    </div>

</template>
