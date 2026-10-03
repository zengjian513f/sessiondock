<script setup lang="ts">
import { computed, watch } from 'vue'
import { operations } from '../../services/conversation/bridge'
import {messageTimeRange} from '../../domain/conversation/planning'
import InnerContent from './InnerContent.vue'
import type { ConversationStore } from '../../stores/conversation'
const props = defineProps<{m:any; store:ConversationStore; actions?:any}>()
const rows = computed<any[]>(() => props.m.questions?.length ? props.m.questions : [{question:props.m.text, options:[]}])
const waiting = computed(() => (props.m.state || 'waiting')==='waiting')
const screen = computed(() => props.m.kind==='screen_menu')
const cli = computed(() => operations().sessiondockCli(props.m.source || props.m.uid))
const form = computed(() => waiting.value && !screen.value && !!cli.value?.canAnswerQuestionForm({...props.m,questions:rows.value}))
const direct = computed(() => waiting.value && rows.value.length===1 && !rows.value[0]?.multiple && !!rows.value[0]?.options?.length)
const state = props.store.disclosure(`question:${props.m.uid || ''}:${props.m.id || props.m.call_id || props.store.messageKey(props.m)}`)
if(form.value) state.selected=operations().questionDraft(props.m,rows.value) || state.selected
if (state.selected.length !== rows.value.length) state.selected = Array(rows.value.length).fill(null)
if (props.m.text && typeof props.m.text==='object') state.text = operations().screenText(props.m) ?? props.m.text.value ?? ''
watch(()=>state.text,value=>{if(screen.value)operations().saveScreenText(props.m,value)})
watch(()=>props.m.state,()=>state.submitting=null)
const cliName = computed(() => cli.value?.name || 'CLI')
const times = computed(()=>messageTimeRange([props.m]))
function answer(uid:string,index:number) { return (props.actions?.answer || operations().answer)(uid,index) }
function cancel(uid:string) { return (props.actions?.cancel || operations().cancel)(uid) }
function action(uid:string,index:number) { return props.actions?.action(uid,index) }
function textAnswer(uid:string,value:string) { return props.actions?.textAnswer(uid,value) }

function selected(i:number,j:number) { return screen.value ? !!rows.value[0]?.options?.[j]?.selected : form.value && state.selected[i]===j }
function disabled(option:any) {
  if (state.submitting!==null) return true
  return screen.value ? !waiting.value || !option?.keys?.length : !direct.value && !form.value
}
async function option(i:number,j:number) {
  if (screen.value) { await answer(props.m.uid,j); return }
  if (form.value) { state.selected[i]=j; return }
  state.submitting=j
  const ok = await answer(props.m.uid,j)
  if (!ok) state.submitting=null
}
async function submit() {
  state.submitting=-1
  const ok=await operations().answerCliQuestionForm(props.m.uid,[...state.selected])
  if (!ok) state.submitting=null
}
</script>
<template>
  <div class="msg question" :class="{'live-question':m.live, settling:m.live && !waiting}" data-role="question" :data-counted="m.counted===false ? 'false' : undefined" :data-call-id="m.call_id || undefined" :data-time-start="times?.start" :data-time-end="times?.end">
    <div class="mb question-body">
      <section v-for="(q,i) in rows" :key="i" class="question-item">
        <InnerContent v-if="q.header" class="question-header" :text="String(q.header)" :syntax="false" />
        <InnerContent class="question-text" :text="String(q.question || m.text)" :syntax="false" />
        <div v-if="q.multiple" class="question-multiple">可多选</div>
        <div v-if="q.options?.length" class="question-options">
          <component :is="m.live ? 'button' : 'div'" v-for="(o,j) in q.options" :key="j" :type="m.live ? 'button' : undefined" class="question-option" :class="{selected:m.live && selected(i,Number(j)), submitting:state.submitting===j && !form}"
            :data-question-index="m.live ? i : undefined" :data-question-option="m.live ? j : undefined" :aria-pressed="m.live ? selected(i,Number(j)) : undefined" :disabled="m.live ? disabled(o) : undefined" @click="m.live && option(i,Number(j))">
            <span>{{Number(j)+1}}</span><div><InnerContent tag="b" :text="String(o.label)" :syntax="false" /><InnerContent v-if="o.description" tag="small" :text="String(o.description)" :syntax="false" /></div>
          </component>
        </div>
      </section>
      <template v-if="m.live && screen">
        <form v-if="m.text && typeof m.text==='object'" class="question-text-form" @submit.prevent="textAnswer(m.uid,state.text)">
          <label>{{m.text.label || '回答'}}<component :is="m.text.multiline ? 'textarea' : 'input'" :type="m.text.multiline ? undefined : 'text'" class="question-text-input" :aria-label="m.text.label || '回答'" :value="state.text" :disabled="!waiting" @input="state.text=($event.target as HTMLInputElement).value" /></label>
          <button type="submit" class="question-submit" :disabled="!waiting">{{m.text.submit_label || '提交文字'}}</button>
        </form>
        <div class="question-actions">
          <small v-if="!waiting" class="question-settling">正在处理，等待终端画面…</small>
          <button v-for="(item,index) in (m.actions || [])" :key="index" type="button" class="question-submit" :data-question-action="index" :disabled="!waiting || !item.keys?.length" @click="action(m.uid,Number(index))">{{item.label}}</button>
          <button type="button" @click="operations().revealNativeTerminal(m.uid)">打开终端</button>
          <button v-if="m.cancel_keys?.length" type="button" class="question-cancel" :disabled="!waiting" @click="cancel(m.uid)">取消</button>
        </div>
      </template>
      <div v-else-if="m.live" class="question-actions">
        <small v-if="!waiting" class="question-settling">{{m.state==='cancelled' ? `正在取消，等待 ${cliName} 记录…` : `答案已提交，等待 ${cliName} 记录…`}}</small>
        <template v-else-if="form"><small>请为每题选择一个答案</small><button type="button" class="question-submit" :class="{submitting:state.submitting===-1}" :disabled="state.submitting!==null || state.selected.some(x=>x===null)" @click="submit">提交答案</button></template>
        <small v-else-if="!direct">多选或多题请在原生终端回答</small>
        <button type="button" :disabled="state.submitting!==null" @click="operations().revealNativeTerminal(m.uid)">打开终端</button>
        <button type="button" class="question-cancel" :disabled="!waiting || state.submitting!==null" @click="cancel(m.uid)">取消</button>
      </div>
    </div>
  </div>
</template>
