<script setup lang="ts">
import { runtimePinia } from '../../stores/runtime/pinia'
import { useRuntimePresentationStore } from '../../stores/runtime/presentation'
import { appController as c } from '../../stores/session-ui'
const button = useRuntimePresentationStore(runtimePinia).consoleButton
function click(event:MouseEvent){c.value!.consoleClick(event.currentTarget)}
</script>
<template><button class="iconbtn" :class="{on:button.on,'console-unavailable':button.unavailable}" id="a-term" type="button"
  :title="button.title" :aria-label="button.label" :aria-expanded="button.expanded" :data-unavailable="String(button.unavailable)"
  @mouseenter="c?.consoleHint()" @focus="c?.consoleHint()" @mouseleave="c?.clearConsoleHint()" @blur="c?.clearConsoleHint()" @click="click"><svg class="ui-icon" aria-hidden="true"><use :href="`#i-${button.icon}`"/></svg></button></template>
