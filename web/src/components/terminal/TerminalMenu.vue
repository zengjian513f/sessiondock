<script setup lang="ts">
import { onMounted, useTemplateRef } from 'vue'
import type { TerminalMenuActions, TerminalMenuElements, TerminalMenuState } from '../../services/terminal/menu-types'
const props = defineProps<{
  state: TerminalMenuState; revision: number
  connect(elements: TerminalMenuElements): TerminalMenuActions
}>()
const menu = useTemplateRef<HTMLDivElement>('menu')
const search = useTemplateRef<HTMLFormElement>('search')
const menuButton = useTemplateRef<HTMLButtonElement>('menuButton')
const query = useTemplateRef<HTMLInputElement>('query')
let actions: TerminalMenuActions
onMounted(() => { actions = props.connect({menu: menu.value!, search: search.value!, menuButton: menuButton.value!, query: query.value!}) })
</script>
<template>
  <div ref="menu" class="ctx-menu term-context-menu" role="menu" :hidden="state.menuHidden" :style="{left: state.left, top: state.top}" @keydown="actions.menuKeydown($event)" @mousedown.stop @mouseup.stop @click.stop @contextmenu.stop>
    <button type="button" role="menuitem" :disabled="state.pasteDisabled" @click="actions.paste()">粘贴</button><button type="button" role="menuitem" @click="actions.showSearch()">查找</button>
  </div>
  <form ref="search" class="term-find" :hidden="state.searchHidden" @submit.prevent="actions.next()" @keydown="actions.searchKeydown($event)" @mousedown.stop @mouseup.stop @click.stop @contextmenu.stop>
    <input ref="query" aria-label="查找终端输出" placeholder="查找已加载输出" type="search" @input="actions.input()"><span role="status">{{state.status}}</span><button type="button" aria-label="上一个" @click="actions.previous()">↑</button><button type="submit" aria-label="下一个">↓</button><button type="button" aria-label="关闭查找" @click="actions.closeSearch()">×</button>
  </form>
  <button ref="menuButton" type="button" class="term-menu-button" aria-label="终端菜单" aria-haspopup="menu" @click.stop="actions.toggleMenu()" @mousedown.stop @mouseup.stop @contextmenu.stop>⋯</button>
</template>
