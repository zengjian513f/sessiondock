<script setup lang="ts">
import type { SettingsState, SettingsBridge } from '../legacy/settings-bridge'
defineProps<{ state: SettingsState; bridge: SettingsBridge }>()
const value = (event: Event) => (event.target as HTMLSelectElement).value
</script>

<template>
    <label class="setting-row" for="setting-sleep">
      <span><b>自动休眠</b><small>无操作时暂停同步，按 Resume 恢复；会话继续运行</small></span>
      <select id="setting-sleep" :value="String(state.sleep)" @change="bridge.sleep(value($event))">
        <option value="5">5 分钟</option>
        <option value="15">15 分钟</option>
        <option value="30">30 分钟</option>
        <option value="60">1 小时（默认）</option>
        <option value="120">2 小时</option>
        <option value="240">4 小时</option>
        <option value="0">不休眠</option>
      </select>
    </label>
    <label class="setting-row" for="setting-cache">
      <span><b>历史缓存</b><small>打开中的会话固定驻留，不计入此上限</small></span>
      <select id="setting-cache" :value="String(state.cache)" @change="bridge.cache(value($event))">
        <option value="64">64 MB</option>
        <option value="128">128 MB</option>
        <option value="256">256 MB</option>
        <option value="512">512 MB</option>
        <option value="1024">1 GB</option>
        <option value="0">不限制</option>
      </select>
    </label>
    <label class="setting-row" for="setting-stop-concurrency">
      <span><b>停止并发数量</b><small>同时停止的会话数，下次批量停止生效</small></span>
      <select id="setting-stop-concurrency" :value="String(state.stopConcurrency)" @change="bridge.stopConcurrency(value($event))">
        <option value="1">1 个（串行）</option>
        <option value="2">2 个</option>
        <option value="4">4 个</option>
        <option value="6">6 个（默认）</option>
        <option value="8">8 个</option>
        <option value="12">12 个</option>
        <option value="16">16 个</option>
      </select>
    </label>
    <label class="setting-row" for="setting-console-paste-files">
      <span><b>控制台粘贴文件</b><small>粘贴的图片或文件存入会话目录 sessiondock_attachments，路径填入终端</small></span>
      <input id="setting-console-paste-files" type="checkbox" :checked="state.pasteFiles" @change="bridge.pasteFiles(($event.target as HTMLInputElement).checked)">
    </label>

</template>
