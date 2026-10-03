<script setup lang="ts">
import ModelPicker from './ModelPicker.vue'
import { sessionUi as ui, launchController as c } from '../../stores/session-ui'
function close() {document.getElementById('new-session-dialog') && (document.getElementById('new-session-dialog') as HTMLDialogElement).close()}
</script>
<template><dialog @pointerdown="c?.newPointerdown($event); c?.NewModels.outside($event)" @pointercancel="c?.newPointercancel" @click="c?.newBackdropClick" @close="c?.newClosed" id="new-session-dialog" aria-labelledby="new-session-title">
  <form @submit="c?.createNewSession" id="new-session-form">
    <div class="modal-head">
      <div><h2 id="new-session-title">新建会话</h2></div>
      <button type="button" @click="close" class="modal-close" title="关闭" aria-label="关闭">×</button>
    </div>
    <div class="new-row">
      <label id="new-node-label" class="node-select" :hidden="!ui.newNodeVisible" title="机器"><span class="visually-hidden">机器</span><select id="new-node" :value="ui.newNode" @change="c?.newNodeChanged"><option v-for="node in ui.newNodes" :key="node.id" :value="node.id" :disabled="node.disabled">{{node.label}}</option></select></label>
      <fieldset class="new-source" @change="c?.newSourceChanged">
        <legend class="visually-hidden">会话类型</legend>
        <label :title="ui.newSources.find(s=>s.value==='claude')?.title || 'Claude'"><input type="radio" name="new-source" value="claude" :checked="ui.newSource==='claude'" :disabled="ui.newSources.find(s=>s.value==='claude')?.disabled"><span><svg class="ico source-icon" data-source="claude" aria-hidden="true"><use href="#i-claude"/></svg><em class="src-label">Claude</em></span></label>
        <label :title="ui.newSources.find(s=>s.value==='codex')?.title || 'Codex'"><input type="radio" name="new-source" value="codex" :checked="ui.newSource==='codex'" :disabled="ui.newSources.find(s=>s.value==='codex')?.disabled"><span><svg class="ico source-icon" data-source="codex" aria-hidden="true"><use href="#i-codex"/></svg><em class="src-label">Codex</em></span></label>
        <label :title="ui.newSources.find(s=>s.value==='grok')?.title || 'Grok'"><input type="radio" name="new-source" value="grok" :checked="ui.newSource==='grok'" :disabled="ui.newSources.find(s=>s.value==='grok')?.disabled"><span><svg class="ico source-icon" data-source="grok" aria-hidden="true"><use href="#i-grok"/></svg><em class="src-label">Grok</em></span></label>
        <label :title="ui.newSources.find(s=>s.value==='opencode')?.title || 'OpenCode'"><input type="radio" name="new-source" value="opencode" :checked="ui.newSource==='opencode'" :disabled="ui.newSources.find(s=>s.value==='opencode')?.disabled"><span><svg class="ico source-icon" data-source="opencode" aria-hidden="true"><use href="#i-opencode"/></svg><em class="src-label">OpenCode</em></span></label>
        <label :title="ui.newSources.find(s=>s.value==='shell')?.title || 'SSH'"><input type="radio" name="new-source" value="shell" :checked="ui.newSource==='shell'" :disabled="ui.newSources.find(s=>s.value==='shell')?.disabled"><span><svg class="ico source-icon" data-source="shell" aria-hidden="true"><use href="#i-terminal"/></svg><em class="src-label">SSH</em></span></label>
      </fieldset>
      <ModelPicker prefix="new" />
    </div>
    <div class="new-cwd-combobox">
      <div class="new-where">
        <div class="new-cwd-field">
          <label class="new-cwd-label" for="new-cwd">启动目录</label>
          <input id="new-cwd" :value="ui.newCwd" @input="c?.cwdInput" @keydown="c?.cwdKeydown" name="cwd" autocomplete="off" spellcheck="false"
            role="combobox" aria-autocomplete="list" aria-haspopup="listbox"
            aria-controls="new-cwd-options" :aria-expanded="ui.cwdVisible" :aria-activedescendant="ui.cwdActive<0?undefined:`new-cwd-option-${ui.cwdActive}`"
            placeholder="/home/user/Projects/project">
        </div>
      </div>
      <div id="new-cwd-picker" :hidden="!ui.cwdVisible">
        <div class="new-cwd-picker-head">
          <span id="new-cwd-options-title">{{ui.cwdTitle}}</span>
          <span>↑↓ 选择 · Tab 补全</span>
        </div>
        <div id="new-cwd-options" role="listbox" aria-labelledby="new-cwd-options-title" @click="c?.cwdChoose">
<template v-for="(row,index) in ui.cwdRows" :key="index"><div v-if="row.section" class="new-cwd-section" role="presentation">{{row.section}}</div><div v-else-if="row.note" class="new-cwd-empty">{{row.note}}</div><button v-else type="button" :id="`new-cwd-option-${row.index}`" class="new-cwd-option" :data-cwd-kind="row.kind" :data-cwd-option="row.index" role="option" :class="{active:ui.cwdActive===row.index}" :aria-selected="ui.cwdActive===row.index" :title="row.path"><span class="new-cwd-option-path">{{row.path}}</span><span v-if="row.meta" class="new-cwd-option-meta">{{row.meta}}</span></button></template></div>
      </div>
      <div id="new-cwd-completion-status" class="visually-hidden" role="status" aria-live="polite">{{ui.cwdStatus}}</div>
    </div>
    <div class="modal-actions">
      <div id="new-session-error" role="alert">{{ui.newError}}</div>
      <button type="button" @click="close" class="btn modal-cancel">取消</button>
      <button type="submit" class="btn go" id="new-session-go" :disabled="ui.newSubmitDisabled">{{ui.newSubmitLabel}}</button>
    </div>
  </form>
</dialog>
</template>
