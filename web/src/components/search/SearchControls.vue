<script setup lang="ts">
import { computed } from 'vue'
import { state } from '../../stores/search'
import { input, keydown, toggleMode, toggleOption } from '../../services/search/runtime'
const modeTitle = computed(() => state.opts.regex ? '正则模式使用整段表达式，不使用 AND / OR' : state.opts.mode === 'any' ? '任一词（OR），点击切换为全部词（AND）' : '全部词（AND），点击切换为任一词（OR）；关键词可在不同消息中')
</script>
<template>
  <div class="qbox">
    <input id="q" :value="state.query" autocomplete="off" aria-label="搜索会话" :placeholder="state.opts.regex ? '正则搜索…  Enter 搜索对话正文' : '搜索… Enter 搜正文'" title="无需回车：筛选标题、UUID / UID、目录、机器名、Agent 名和模型名；空格分词，双引号搜索短语；Enter 搜索对话正文" @input="input(($event.target as HTMLInputElement).value)" @keydown="keydown">
    <div class="opts" id="search-mode" role="group" aria-label="关键词匹配方式">
      <button type="button" id="search-mode-toggle" class="on" :disabled="state.opts.regex" :title="modeTitle" :aria-label="modeTitle" @click="toggleMode">{{ state.opts.mode === 'any' ? 'OR' : 'AND' }}</button>
    </div>
    <div class="opts" id="opts">
      <button type="button" data-o="case" title="大小写敏感" aria-label="大小写敏感" :class="{on: state.opts.case}" :aria-pressed="!!state.opts.case" @click="toggleOption('case')">Aa</button>
      <button type="button" data-o="word" title="全词匹配" aria-label="全词匹配" :class="{on: state.opts.word}" :aria-pressed="!!state.opts.word" @click="toggleOption('word')">ab|</button>
      <button type="button" data-o="regex" title="正则表达式：整段输入作为正则，不拆词" aria-label="正则表达式" :class="{on: state.opts.regex}" :aria-pressed="!!state.opts.regex" @click="toggleOption('regex')">.*</button>
    </div>
  </div>
</template>
