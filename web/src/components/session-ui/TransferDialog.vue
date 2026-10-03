<script setup lang="ts">
import {computed} from 'vue'
import { sessionUi as ui } from '../../stores/session-ui'
const t=computed(()=>ui.transfer!)
</script>
<template><dialog id="clone-group-dialog" class="app-dialog transfer-dialog" aria-labelledby="transfer-title" :aria-busy="t.busy" @cancel="t.actions.cancel">
    <div class="transfer-head">
      <div><h2 id="transfer-title">移动或复制会话组</h2></div>
      <button @click="t.actions.close()" class="transfer-close" :disabled="t.aborting" type="button" aria-label="关闭">×</button>
    </div>
    <div class="transfer-body">
      <div class="transfer-controls">
        <label class="transfer-field"><span>源机器</span><input id="transfer-source" :value="t.sourceName" type="text" disabled></label>
        <label class="transfer-field"><span>目标机器</span><select id="transfer-target" :value="t.targetNode" :disabled="t.controlsDisabled" @change="t.actions.targetChanged"><option v-for="o in t.options" :key="o.value" :value="o.value" :disabled="o.disabled">{{o.label}}</option></select></label>
        <fieldset class="transfer-mode"><legend>操作</legend><div class="transfer-segments">
          <label><input type="radio" @change="t.actions.modeChanged" name="transfer-mode" value="clone" :checked="t.mode==='clone'" :disabled="t.controlsDisabled"><span>复制</span></label>
          <label><input type="radio" @change="t.actions.modeChanged" name="transfer-mode" value="move" :checked="t.mode==='move'" :disabled="t.controlsDisabled"><span>移动</span></label>
        </div></fieldset>
      </div>
      <div class="transfer-identity" :hidden="!t.identityVisible">
        <label><input @change="t.actions.identityChanged" id="transfer-new-ids" type="checkbox" :checked="t.newIds" :disabled="t.controlsDisabled"><span>生成新 UID</span></label>
      </div>
      <p class="transfer-notice" role="status" :hidden="!t.notice">{{t.notice}}</p>
      <p class="transfer-environment" role="status" :hidden="!t.environmentVisible" :title="t.environmentTitle">{{t.environmentText}}</p>
      <div class="transfer-section-head"><h3>整组会话</h3><span class="clone-status" role="status">{{t.status}}</span></div>
      <div class="transfer-table-scroll" tabindex="0" role="region" aria-label="整组会话清单">
        <table class="clone-members"><thead><tr><th scope="col">会话</th><th scope="col">来源</th><th scope="col">关联</th><th scope="col" class="transfer-number">历史文件</th><th scope="col" class="transfer-number">大小</th></tr></thead>
          <tbody><tr v-if="!t.members.length"><td colspan="5" class="transfer-empty">正在检查关联会话和历史依赖…</td></tr><tr v-for="m in t.members" :key="`${m.source}:${m.sid}`" :class="{'transfer-selected':m.selected}"><td><div class="transfer-session-name" :title="m.name">{{m.name}}</div><div class="transfer-session-detail" :title="m.detailTitle">{{m.detail}}</div></td><td>{{m.sourceName}}</td><td><span class="transfer-badge">{{m.badge}}</span></td><td class="transfer-number">{{m.files}}</td><td class="transfer-number">{{m.sizeLabel}}</td></tr></tbody></table>
      </div>
      <p class="transfer-progress" role="status" :hidden="!t.progressVisible" :data-phase="t.progressPhase">{{t.progressText}}</p>
      <p class="transfer-error" role="alert" :hidden="!t.errorVisible">{{t.errorText}}</p>
    </div>
    <div class="transfer-footer"><button type="button" @click="t.actions.close()" class="btn clone-cancel" :disabled="t.aborting">取消</button><button type="button" @click="t.actions.abort()" class="btn transfer-abort" :hidden="!t.abortVisible" :disabled="t.aborting">{{t.abortLabel}}</button><button type="button" @click="t.actions.confirm()" class="btn primary clone-confirm" :disabled="t.confirmDisabled">{{t.confirmLabel}}</button></div></dialog></template>