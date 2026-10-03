<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted } from 'vue'
import HeaderAction from './HeaderAction.vue'
import { state, actionDefinitions } from '../../stores/shell'
import { closeMenu } from '../../services/shell/header'
const inline = computed(() => actionDefinitions.filter(a => !state.folded.includes(a.id)))
const folded = computed(() => actionDefinitions.filter(a => state.folded.includes(a.id)))
const items = () => [...document.querySelectorAll<HTMLButtonElement>('#header-menu button:not(:disabled):not(.hidden)')]
async function openKey(event: KeyboardEvent) {
  if (!['ArrowDown', 'ArrowUp'].includes(event.key)) return
  event.preventDefault(); state.menuOpen = true; await nextTick()
  const rows = items(); (event.key === 'ArrowUp' ? rows.at(-1) : rows[0])?.focus()
}
function menuKey(event: KeyboardEvent) {
  const rows = items(), index = rows.indexOf(document.activeElement as HTMLButtonElement)
  const next: Record<string, number> = {ArrowDown: (index + 1) % rows.length,
    ArrowUp: (index - 1 + rows.length) % rows.length, Home: 0, End: rows.length - 1}
  if (next[event.key] === undefined) return
  event.preventDefault(); rows[next[event.key]!]?.focus()
}
function focusOut(event: FocusEvent) {
  if (event.relatedTarget && !(event.currentTarget as Element).contains(event.relatedTarget as Node)) closeMenu()
}
function outside(event: MouseEvent) {
  if (!(event.target as Element).closest('#header-more')) closeMenu()
}
function escape(event: KeyboardEvent) {
  if (event.key === 'Escape' && state.menuOpen) {
    event.preventDefault(); event.stopImmediatePropagation(); closeMenu(true)
  }
}
onMounted(() => {
  document.addEventListener('click', outside, true); document.addEventListener('keydown', escape, true)
})
onUnmounted(() => {
  document.removeEventListener('click', outside, true); document.removeEventListener('keydown', escape, true)
})
</script>
<template>
  <div class="header-actions">
    <HeaderAction v-for="action in inline" :key="action.id" v-bind="action" :folded="false" />
    <div class="header-more" id="header-more" :hidden="!folded.length" @focusout="focusOut">
      <button type="button" class="btn" id="header-more-btn" title="更多操作" aria-label="更多操作" aria-haspopup="menu"
        :aria-expanded="state.menuOpen ? 'true' : 'false'" aria-controls="header-menu" @click="state.menuOpen = !state.menuOpen" @keydown="openKey"><span aria-hidden="true">⋯</span></button>
      <div id="header-menu" class="header-menu" :hidden="!state.menuOpen" role="menu" aria-label="更多操作" @click="closeMenu(true)" @keydown="menuKey">
        <HeaderAction v-for="action in folded" :key="action.id" v-bind="action" :folded="true" />
      </div>
    </div>
  </div>
</template>
