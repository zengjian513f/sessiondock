<script setup lang="ts">
import { computed } from 'vue'
import type {ModelPresentation} from '../../domain/session-ui/types'
import { sessionUi as ui, launchController } from '../../stores/session-ui'
const props = defineProps<{prefix: string}>()
const picker = computed(() => props.prefix === 'new' ? launchController.value?.NewModels : launchController.value?.BugReportModels)
const view = computed<ModelPresentation>(() => ui.models[props.prefix] || {rows:[],efforts:[],model:'',effort:'',label:'读取模型…',title:'模型',enabled:false,effortTitle:'推理强度',open:false,searchVisible:false,query:'',active:-1})
</script>
<template><div class="new-choice"><div class="new-model">
<button type="button" :id="`${prefix}-model`" class="new-pick" :title="view.title" :disabled="!view.enabled" aria-haspopup="listbox" :aria-expanded="view.open" :aria-controls="`${prefix}-model-menu`" @click="picker?.toggle()" @keydown="picker?.buttonKey"><span :id="`${prefix}-model-label`" :class="{default:!view.enabled}">{{view.label}}</span><i aria-hidden="true">⌄</i></button>
<div :id="`${prefix}-model-menu`" class="new-model-menu" popover="manual" :hidden="!view.open" @keydown="picker?.keydown">
<input :id="`${prefix}-model-search`" class="new-model-search" type="search" placeholder="搜索模型" aria-label="搜索模型" autocomplete="off" spellcheck="false" :aria-controls="`${prefix}-model-options`" :hidden="!view.searchVisible" :value="view.query" :aria-activedescendant="view.active<0?undefined:`${prefix}-model-option-${view.active}`" @input="picker?.search($event)">
<div :id="`${prefix}-model-options`" class="new-model-options" role="listbox" aria-label="模型" tabindex="-1" :aria-activedescendant="view.active<0?undefined:`${prefix}-model-option-${view.active}`">
<button v-for="(model,index) in view.rows" :key="model.id" type="button" :id="`${prefix}-model-option-${index}`" class="new-model-option" :class="{active:index===view.active}" :data-model-option="index" role="option" :aria-selected="model.id === view.model" tabindex="-1" :title="model.id" @click="picker?.choose(index)"><span>{{model.name || model.id}}</span><small v-if="model.id && model.name && model.name!==model.id">{{model.id}}</small></button><div v-if="!view.rows.length" class="new-model-empty">没有匹配的模型</div>
</div></div></div><label class="new-effort" :title="view.effortTitle"><span class="visually-hidden">推理强度</span><select :id="`${prefix}-effort`" :value="view.effort" :disabled="!view.efforts.length" @change="picker?.effortChanged"><option v-for="effort in view.efforts" :key="effort" :value="effort">{{effort}}</option></select></label></div></template>
