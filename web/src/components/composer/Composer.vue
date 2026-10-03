<script setup lang="ts">
import { onUpdated } from 'vue'
import { composerState as state, composerOperations as operations } from '../../stores/composer'
import { attachmentIcon, attachmentSummary } from '../../domain/composer/presentation'
import ComposerQuestion from './ComposerQuestion.vue'
function summary(a:any) {return attachmentSummary(a,()=>operations().attachmentMeta(a))}
onUpdated(()=>operations().measureEditor())
</script>
<template>
  <div id="composer" :class="{hidden:!state.visible,dragover:state.dragover}" @dragenter="operations().dragenter($event)" @dragover="operations().dragover($event)" @dragleave="operations().dragleave($event)" @drop="operations().drop($event)">
    <div id="composer-question" :class="{hidden:!state.question}" aria-label="CLI 选择题" :data-signature="state.signature"><ComposerQuestion v-if="state.question" :key="`${state.question.uid}\0${state.question.id}`" /></div>
    <div id="composer-input-status" :class="{hidden:!state.notice,blocked:state.blocked,'input-attention':!!state.attention,'input-question':state.attention==='question'}" role="status" aria-live="polite" aria-atomic="true" :title="state.notice"><span v-if="state.notice" class="composer-input-status-copy">{{state.notice}}</span></div>
    <div id="compose-items" aria-live="polite">
      <div v-for="a in state.attachments" :key="a.id" class="draft-card" :class="a.status" :data-draft-id="a.id" :title="`点击插入 [附件${a.number}]`" @click="operations().attachmentInsert(a.number)">
        <span class="draft-thumb"><img v-if="a.kind==='image' && a.preview" :src="a.preview" alt=""><template v-else>{{attachmentIcon(a.kind)}}</template></span>
        <span class="draft-info"><b>{{a.uploaded?.name || a.file?.name || 'attachment'}}</b><small>{{summary(a)}}</small><button v-if="a.status==='failed' && operations().attachmentCanRetry(a)" type="button" class="draft-retry" title="重新上传" :disabled="state.sending" @click.stop="operations().attachmentRetry(a.id)">重试</button></span>
        <button type="button" class="draft-remove" :title="a.cancelUpload ? '取消上传' : '移除附件'" :aria-label="a.cancelUpload ? '取消上传' : '移除附件'" :disabled="state.sending && !a.cancelUpload" @click.stop="operations().attachmentRemove(a.id)">×</button>
      </div>
      <div v-for="q in state.quotes" :key="q.id" class="draft-card draft-quote" :data-draft-id="q.id"><span>❝</span><textarea :value="q.text" maxlength="16000" placeholder="粘贴或输入要引用的文字" aria-label="引用文字" @input="operations().quoteInput(q.id,$event)" /><button type="button" class="draft-remove" title="移除引用" aria-label="移除引用" :disabled="state.sending" @click="operations().removeComposerQuote(q.id)">×</button></div>
      <button v-if="state.restartable" type="button" class="btn" data-conversation-restart :disabled="state.restarting" @click="operations().restartComposer()">重新启动</button>
      <div v-if="state.storageError" class="draft-save-error" role="alert">{{state.storageError}}</div>
    </div>
    <div class="composer-row">
      <div class="attach-picker">
        <button class="btn" :class="{on:state.menu}" id="cadd" type="button" title="添加附件或引用" aria-label="添加附件或引用" :aria-expanded="(state.menu ? 'true' : 'false')" :disabled="state.addDisabled" @click="operations().toggleAttachments($event)"><svg class="ui-icon" aria-hidden="true"><use href="#i-plus"/></svg></button>
        <div class="attach-menu" :class="{hidden:!state.menu}" id="attach-menu" role="menu">
          <button type="button" data-attach="image" role="menuitem" @click="operations().chooseAttachment('image')"><span>▧</span>图片</button>
          <button type="button" data-attach="video" role="menuitem" @click="operations().chooseAttachment('video')"><span>▶</span>视频</button>
          <button type="button" data-attach="audio" role="menuitem" @click="operations().chooseAttachment('audio')"><span>♪</span>音频</button>
          <button type="button" data-attach="file" role="menuitem" @click="operations().chooseAttachment('file')"><span>⌑</span>文件</button>
          <button type="button" data-attach="quote" role="menuitem" @click="operations().chooseAttachment('quote')"><span>❝</span>引用文字</button>
        </div>
        <input id="cfile" type="file" multiple hidden @change="operations().filesChanged($event)">
      </div>
      <div class="composer-input-wrap">
        <div id="input-history" class="input-history" :class="{hidden:!state.history.open}" role="listbox" aria-label="输入历史">
          <template v-if="state.history.open"><div class="input-history-head"><span>输入历史</span><span class="input-history-position">{{state.history.state==='loading' ? '加载中…' : `${Math.max(0,state.history.index+1)} / ${state.history.items.length}${state.history.state==='refreshing' ? ' · 加载全部…' : ''}`}}</span></div>
          <div v-if="state.history.state==='loading' || !state.history.items.length" class="input-history-empty">{{state.history.error || (state.history.state==='loading' ? '正在加载输入历史…' : '暂无输入历史')}}</div>
          <div v-else class="input-history-list"><button v-for="(item,index) in state.history.items" :key="index" type="button" :id="`input-history-option-${index}`" class="input-history-item" :class="{selected:state.history.index===index}" :data-history-index="index" role="option" :aria-selected="(state.history.index===index ? 'true' : 'false')" @mouseenter="operations().setComposerHistoryIndex(index)" @mousedown.prevent @click="operations().setComposerHistoryIndex(index);operations().acceptComposerHistory()"><span class="input-history-text">{{item.text.slice(0,600)}}</span><small>{{operations().historyTime(item.ts,index)}}</small></button></div></template>
        </div>
        <textarea id="cinput" rows="1" enterkeyhint="enter" :placeholder="state.placeholder" :value="state.text" :disabled="state.loading" aria-controls="input-history" :aria-expanded="(state.history.open ? 'true' : 'false')" :aria-activedescendant="state.history.open && state.history.index>=0 ? `input-history-option-${state.history.index}` : undefined" @input="operations().input($event)" @keydown="operations().keydown($event)" @paste="operations().paste($event)" />
      </div>
      <div class="cbtns"><button class="btn" id="cesc" title="单击向 CLI 发送 Esc；Claude 中双击进入原生回滚选择" aria-label="单击发送 Esc；Claude 中双击进入回滚选择" @click="operations().sendComposerEscape()">Esc</button><button class="btn go" id="csend" :disabled="state.sendDisabled" :aria-busy="(!!state.busy ? 'true' : 'false')" :aria-label="state.busy || undefined" @mousedown="operations().sendMouseDown($event)" @click="operations().submitComposer()">发送</button></div>
    </div>
  </div>
</template>
