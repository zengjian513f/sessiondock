<script setup lang="ts">
import type { PickBar, SidebarActions } from '../../domain/sidebar/types'
defineProps<{view: PickBar; actions: SidebarActions}>()
</script>
<template>
  <span id="side-picked">{{ view.label }}</span>
  <button type="button" class="btn" id="side-pick-all" :hidden="view.attaching" :disabled="view.allDisabled" @click="actions.pickAll()">{{ view.allLabel }}</button>
  <button type="button" class="btn" id="side-pick-stop" :hidden="view.attaching" :disabled="view.stopDisabled" :aria-busy="view.stopBusy" :title="view.stopTitle" @click="actions.pickStop()">{{ view.stopLabel }}</button>
  <button type="button" class="btn" id="side-pick-group" aria-controls="session-group-menu" aria-haspopup="menu" aria-expanded="false" :hidden="view.groupHidden" :disabled="view.groupDisabled" @click="actions.pickGroup($event)">分组</button>
  <button type="button" class="btn" id="side-pick-attach" :hidden="view.attachHidden" :disabled="view.attachDisabled" @click="actions.pickAttach()">{{ view.attachLabel }}</button>
  <button type="button" class="btn danger" id="side-pick-delete" :hidden="view.attaching" :disabled="view.deleteDisabled" @click="actions.pickDelete()">{{ view.deleteLabel }}</button>
  <button type="button" class="btn" id="side-pick-cancel" :disabled="view.cancelDisabled" @click="actions.pickCancel()">取消</button>
  <details id="side-stop-details" :hidden="view.detailsHidden"><summary id="side-stop-summary">{{ view.summary }}</summary><div id="side-stop-errors">{{ view.errors }}</div></details>
</template>
