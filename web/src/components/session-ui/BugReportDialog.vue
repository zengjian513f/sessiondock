<script setup lang="ts">
import ModelPicker from './ModelPicker.vue'
import ReportAttachments from './ReportAttachments.vue'
import { sessionUi as ui, launchController as c } from '../../stores/session-ui'
function close() {document.getElementById('bug-report-dialog') && (document.getElementById('bug-report-dialog') as HTMLDialogElement).close()}
</script>
<template><dialog @pointerdown="c?.bugPointerdown" @pointercancel="c?.bugPointercancel" @click="c?.bugClick" @close="c?.bugClosed" id="bug-report-dialog" class="app-dialog" aria-labelledby="bug-report-title">
  <form @submit="c?.submitBugReport" @paste="c?.bugPaste" id="bug-report-form" class="report-form">
    <div class="modal-head">
      <div><h2 id="bug-report-title">报告问题</h2></div>
      <button type="button" @click="close" class="modal-close" title="关闭" aria-label="关闭">×</button>
    </div>
    <div class="new-row report-row" role="group" aria-label="处理会话">
      <label id="bug-report-node-label" class="node-select" :hidden="!ui.bugNodeVisible" title="处理这份报告的会话运行在哪台机器"><span class="visually-hidden">处理节点</span><select id="bug-report-node" :value="ui.bugNode" :disabled="ui.bugNodeDisabled" @change="c?.bugNodeChanged"><option v-for="node in ui.bugNodes" :key="node.id" :value="node.id" :disabled="node.disabled">{{node.label}}</option></select></label>
      <fieldset class="new-source report-source" id="bug-report-source" @change="c?.bugSourceChanged">
        <legend class="visually-hidden">处理会话类型</legend>
        <label title="Claude"><input type="radio" name="bug-report-source" value="claude" :checked="ui.bugSource==='claude'" :disabled="ui.bugSources.find(s=>s.value==='claude')?.disabled" :title="ui.bugSources.find(s=>s.value==='claude')?.title"><span><svg class="ico source-icon" data-source="claude" aria-hidden="true"><use href="#i-claude"/></svg><em class="src-label">Claude</em></span></label>
        <label title="Codex"><input type="radio" name="bug-report-source" value="codex" :checked="ui.bugSource==='codex'" :disabled="ui.bugSources.find(s=>s.value==='codex')?.disabled" :title="ui.bugSources.find(s=>s.value==='codex')?.title"><span><svg class="ico source-icon" data-source="codex" aria-hidden="true"><use href="#i-codex"/></svg><em class="src-label">Codex</em></span></label>
        <label title="Grok"><input type="radio" name="bug-report-source" value="grok" :checked="ui.bugSource==='grok'" :disabled="ui.bugSources.find(s=>s.value==='grok')?.disabled" :title="ui.bugSources.find(s=>s.value==='grok')?.title"><span><svg class="ico source-icon" data-source="grok" aria-hidden="true"><use href="#i-grok"/></svg><em class="src-label">Grok</em></span></label>
        <label title="OpenCode"><input type="radio" name="bug-report-source" value="opencode" :checked="ui.bugSource==='opencode'" :disabled="ui.bugSources.find(s=>s.value==='opencode')?.disabled" :title="ui.bugSources.find(s=>s.value==='opencode')?.title"><span><svg class="ico source-icon" data-source="opencode" aria-hidden="true"><use href="#i-opencode"/></svg><em class="src-label">OpenCode</em></span></label>
      </fieldset>
      <ModelPicker prefix="bug-report" />
    </div>
    <div id="bug-report-items" class="report-items" aria-live="polite"><ReportAttachments /></div>
    <div class="composer-row report-composer" :style="{paddingTop:ui.bugAttachPadding}">
      <div class="attach-picker">
        <button class="btn" id="bug-report-add" @click="c?.bugAttachToggle" type="button" title="添加附件" aria-label="添加附件" :aria-expanded="ui.bugAttachOpen" :class="{on:ui.bugAttachOpen}" :disabled="ui.bugAddDisabled">
          <svg class="ui-icon" aria-hidden="true"><use href="#i-plus"/></svg>
        </button>
        <div class="attach-menu" :class="{hidden:!ui.bugAttachOpen}" id="bug-report-attach-menu" role="menu">
          <button type="button" data-attach="image" @click="c?.bugChooseFile('image')" role="menuitem"><span>▧</span>图片</button>
          <button type="button" data-attach="video" @click="c?.bugChooseFile('video')" role="menuitem"><span>▶</span>视频</button>
          <button type="button" data-attach="audio" @click="c?.bugChooseFile('audio')" role="menuitem"><span>♪</span>音频</button>
          <button type="button" data-attach="file" @click="c?.bugChooseFile('file')" role="menuitem"><span>⌑</span>文件</button>
        </div>
        <input id="bug-report-file" @change="c?.bugFileChanged" type="file" multiple hidden>
      </div>
      <div class="composer-input-wrap">
        <textarea id="bug-report-description" :value="ui.bugText" :disabled="ui.bugInputDisabled" @input="c?.bugInput" @keydown="c?.bugKeydown" rows="1" maxlength="50000" enterkeyhint="enter"
          aria-label="问题描述" placeholder="描述遇到的问题"></textarea>
      </div>
      <div class="cbtns">
        <button type="submit" class="btn go" id="bug-report-go" title="提交并启动处理会话" :disabled="ui.bugSubmitDisabled" :aria-busy="!!ui.bugBusyLabel" :aria-label="ui.bugBusyLabel||undefined">{{ui.bugSubmitLabel}}</button>
      </div>
    </div>
    <div id="bug-report-error" role="alert">{{ui.bugError}}</div>
  </form>
</dialog>
</template>
