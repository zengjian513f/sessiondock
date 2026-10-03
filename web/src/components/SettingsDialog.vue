<script setup lang="ts">
import SettingsAppearance from './SettingsAppearance.vue'
import SettingsFeatures from './SettingsFeatures.vue'
import SettingsMachines from './SettingsMachines.vue'
import { state, bridge, tab, selectTab, machines, machineState } from '../migration/settings'
const tabs = [['appearance', '外观'], ['features', '功能'], ['machines', '机器']] as const
function outside(event: MouseEvent) {
  if (event.target === event.currentTarget) (event.currentTarget as HTMLDialogElement).close()
}
</script>

<template>
<dialog id="settings-dialog" class="app-dialog" aria-labelledby="settings-title" @click="outside">
  <form method="dialog" class="settings-form">
    <div class="modal-head">
      <div><h2 id="settings-title">设置</h2><p id="settings-sub">{{ tab === 'machines' ? '机器设置保存在中央服务端，所有浏览器一致' : tab === 'features' ? '功能偏好保存在此浏览器' : '界面偏好保存在浏览器' }}</p></div>
      <button type="submit" class="modal-close" title="关闭" aria-label="关闭">×</button>
    </div>
    <div class="settings-tabs" role="tablist" aria-label="设置分类">
      <button v-for="[name, label] in tabs" :key="name" type="button" class="settings-tab" :class="{on: tab === name}" role="tab"
        :aria-selected="tab === name ? 'true' : 'false'" :aria-controls="`settings-${name}`" :data-tab="name" @click="selectTab(name)">{{ label }}</button>
    </div>
    <div id="settings-appearance" class="settings-pane" role="tabpanel" :hidden="tab !== 'appearance'"><SettingsAppearance :state="state" :bridge="bridge" /></div>
    <div id="settings-features" class="settings-pane" role="tabpanel" :hidden="tab !== 'features'"><SettingsFeatures :state="state" :bridge="bridge" /></div>
    <div id="settings-machines" class="settings-pane" role="tabpanel" :hidden="tab !== 'machines'">
      <SettingsMachines v-if="machines && machineState" :state="machineState" :controller="machines" />
    </div>
    <div class="modal-actions"><button type="submit" class="btn go">完成</button></div>
  </form>
</dialog>
</template>
