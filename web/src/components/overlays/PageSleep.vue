<script setup lang="ts">
import { onMounted, ref } from 'vue'
import type { SleepController } from '../../services/overlays/sleep'
const { controller } = defineProps<{controller: SleepController}>()
const dialog = ref<HTMLDialogElement>()
onMounted(() => controller.attach(dialog.value!))
</script>
<template><dialog id="page-sleep-dialog" ref="dialog" class="page-sleep-dialog" aria-labelledby="page-sleep-title" @cancel.prevent @keydown.stop><div class="app-float session-freeze-line">
<span id="page-sleep-title">页面已休眠</span>
<button class="btn" type="button" aria-label="Resume" @click="controller.resume"><svg class="ui-icon" aria-hidden="true"><use href="#i-play" /></svg><span>Resume</span></button>
</div></dialog></template>
