<script setup lang="ts">
import { computed } from 'vue'
import { composerState as state, composerOperations as operations } from '../../stores/composer'
const m=computed(()=>state.question)
const rows=computed<any[]>(()=>Array.isArray(m.value.questions) && m.value.questions.length ? m.value.questions : [{question:m.value.text,options:[]}])
const waiting=computed(()=>(m.value.state || 'waiting')==='waiting')
const screen=computed(()=>m.value.kind==='screen_menu')
const cli=computed(()=>operations().cliInfo(m.value))
const form=computed(()=>waiting.value && !screen.value && !!cli.value?.canAnswerQuestionForm({...m.value,questions:rows.value}))
const direct=computed(()=>waiting.value && rows.value.length===1 && !rows.value[0]?.multiple && !!rows.value[0]?.options?.length)
function selected(i:number,j:number) {return screen.value ? !!rows.value[0]?.options?.[j]?.selected : form.value && state.questionUi.selected[i]===j}
function disabled(option:any) {return screen.value ? !waiting.value || !option?.keys?.length : state.questionUi.submitting!==null || (!direct.value && !form.value)}
</script>
<template>
  <div class="msg question live-question" :class="{settling:!waiting}" data-role="question" :data-counted="m.counted===false ? 'false' : undefined" :data-call-id="m.call_id">
    <div class="mb question-body">
      <section v-for="(q,i) in rows" :key="i" class="question-item"><div v-if="q.header" class="question-header">{{q.header}}</div><div class="question-text">{{q.question || m.text}}</div><div v-if="q.multiple" class="question-multiple">可多选</div>
        <div v-if="q.options?.length" class="question-options"><button v-for="(o,j) in q.options" :key="j" type="button" class="question-option" :class="{selected:selected(i,Number(j)),submitting:!screen && !form && state.questionUi.submitting===j}" :data-question-index="i" :data-question-option="j" :aria-pressed="(selected(i,Number(j)) ? 'true' : 'false')" :disabled="disabled(o)" @click="operations().questionOption(i,j)"><span>{{Number(j)+1}}</span><div><b>{{o.label}}</b><small v-if="o.description">{{o.description}}</small></div></button></div>
      </section>
      <template v-if="screen">
        <form v-if="m.text && typeof m.text==='object'" class="question-text-form" @submit.prevent="operations().questionTextSubmit()"><label>{{m.text.label || '回答'}}<component :is="m.text.multiline ? 'textarea' : 'input'" :type="m.text.multiline ? undefined : 'text'" class="question-text-input" :aria-label="m.text.label || '回答'" :value="state.questionUi.text" :disabled="!waiting" @input="operations().questionText($event)" /></label><button type="submit" class="question-submit" :disabled="!waiting">{{m.text.submit_label || '提交文字'}}</button></form>
        <div class="question-actions"><small v-if="!waiting" class="question-settling">正在处理，等待终端画面…</small><button v-for="(item,index) in (m.actions || [])" :key="index" type="button" class="question-submit" :data-question-action="index" :disabled="!waiting || !item.keys?.length" @click="operations().questionAction(index)">{{item.label}}</button><button type="button" @click="operations().questionTerminal()">打开终端</button><button v-if="m.cancel_keys?.length" type="button" class="question-cancel" :disabled="!waiting" @click="operations().questionCancel()">取消</button></div>
      </template>
      <div v-else class="question-actions"><small v-if="!waiting" class="question-settling">{{m.state==='cancelled' ? `正在取消，等待 ${cli?.name || 'CLI'} 记录…` : `答案已提交，等待 ${cli?.name || 'CLI'} 记录…`}}</small><template v-else-if="form"><small>请为每题选择一个答案</small><button type="button" class="question-submit" :class="{submitting:state.questionUi.submitting===-1}" :disabled="state.questionUi.submitting!==null || state.questionUi.selected.some((x:any)=>x===null)" @click="operations().questionSubmit()">提交答案</button></template><small v-else-if="!direct">多选或多题请在原生终端回答</small><button type="button" :disabled="state.questionUi.submitting!==null" @click="operations().questionTerminal()">打开终端</button><button type="button" class="question-cancel" :disabled="!waiting || state.questionUi.submitting!==null" @click="operations().questionCancel()">取消</button></div>
    </div>
  </div>
</template>
