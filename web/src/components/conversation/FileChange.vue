<script setup lang="ts">
import { computed } from 'vue'
import { CHANGE_LABEL, diffKind, diffCodeParts, diffSides } from '../../domain/conversation/planning'
import InnerContent from './InnerContent.vue'
import type { ConversationStore } from '../../stores/conversation'
const props = defineProps<{change:any; store:ConversationStore; id:string}>()
const state = props.store.disclosure(props.id)
const path = computed(() => props.change.new_path || props.change.path || '')
const label = computed(() => props.change.new_path ? `${props.change.path} → ${props.change.new_path}` : props.change.path)
const complete = computed(() => props.change.before_complete || props.change.after_complete)
const lines = computed(() => String(props.change.patch || '').split('\n').map(line => ({line, kind:diffKind(line), parts:diffCodeParts(line,diffKind(line))})))
const sides = computed(() => diffSides(props.change))
</script>
<template>
  <section class="file-change-card" :class="{'diff-wrap':state.wrap}" :data-diff-view="state.view" :data-diff-wrap="String(state.wrap)">
    <div class="file-change-head"><b :title="label">{{label}}</b>
      <span class="file-change-meta"><em :title="complete ? '包含可确定的完整文件内容' : '会话只记录了修改片段'">{{(CHANGE_LABEL as Record<string,string>)[change.operation] || '修改'}} · {{complete ? '完整' : '片段'}}</em>
        <i class="add">+{{change.added || 0}}</i><i class="del">−{{change.removed || 0}}</i>
        <span class="file-change-toolbar" role="group" aria-label="Diff 显示选项">
          <button v-for="view in ['unified','split']" :key="view" type="button" :data-diff-view="view" :class="{on:state.view===view}" :aria-pressed="state.view===view" @click="state.view=view">{{view==='unified' ? '统一' : '并排'}}</button>
          <button type="button" data-diff-wrap :class="{on:state.wrap}" :aria-pressed="state.wrap" :title="state.wrap ? '保持 diff 原始行宽' : '长行自动换行'" @click="state.wrap=!state.wrap">{{state.wrap ? '原行' : '换行'}}</button>
        </span>
      </span>
    </div>
    <div class="file-change-body">
      <div v-if="state.view==='unified'" class="diff-unified">
        <div v-for="(row,index) in lines" :key="index" class="diff-line" :class="row.kind"><i>{{row.parts?.marker ?? row.line[0] ?? ' '}}</i><InnerContent tag="code" :text="row.parts?.source ?? row.line" :data-code-path="row.parts && path ? path : undefined" /></div>
      </div>
      <div v-else class="diff-split">
        <section v-for="side in (['before','after'] as const)" :key="side"><b>{{side==='before' ? '修改前' : '修改后'}}{{change[`${side}_complete`] ? '（完整）' : '（片段）'}}</b>
          <div><div v-if="!change[`${side}_available`]" class="diff-unavailable">原内容没有记录，无法可靠还原</div>
            <template v-else-if="sides[side].length"><div v-for="(row,index) in sides[side]" :key="index" class="diff-line" :class="row.kind"><InnerContent tag="code" :text="row.text" :data-code-path="row.kind==='meta' ? undefined : side==='before' ? change.path || path : path" /></div></template>
            <div v-else class="diff-empty">（空文件）</div>
          </div>
        </section>
      </div>
    </div>
  </section>
</template>
