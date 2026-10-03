<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useOverlaysStore } from '../../stores/overlays'
import { fileActions } from '../../services/overlays/files'
import type { FileMenuController } from '../../services/overlays/files'
const { controller } = defineProps<{controller: FileMenuController}>()
const state = useOverlaysStore()
const menu = ref<HTMLElement>()
onMounted(() => controller.attach(menu.value!))
</script>
<template><div id="file-menu" ref="menu" class="ctx-menu" :hidden="!state.fileMenuOpen" role="menu" :aria-label="state.fileMenu.label" aria-describedby="file-menu-target" :style="{left: state.fileMenu.left+'px', top: state.fileMenu.top+'px'}" @keydown="controller.keydown"><div id="file-menu-target" class="ctx-menu-target" dir="auto">{{ state.fileMenu.text }}</div><button v-for="[action, label] in fileActions" :key="action" type="button" :data-action="action" role="menuitem" :hidden="!state.fileMenu.actions.includes(action)" @click="controller.act(action)">{{ label }}</button></div></template>
