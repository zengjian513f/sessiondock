<script setup lang="ts">
import { state, invoke } from '../../stores/shell'
import type { ActionId } from '../../stores/shell'
defineProps<{id: ActionId; label: string; icon: string; folded: boolean}>()
</script>
<template>
  <button :type="id === 'page-reload' ? 'button' : undefined" class="btn" :class="{hidden: state.actions[id].capabilityHidden}"
    :id="id" :hidden="state.actions[id].hidden" :disabled="state.actions[id].disabled" :title="label" :aria-label="label"
    :data-report-bug="id === 'report-bug' ? '' : undefined" :data-session-docked="state.actions[id].docked ? '' : undefined"
    :role="folded ? 'menuitem' : undefined" @click.capture="invoke(id, $event)">
    <svg class="ui-icon" aria-hidden="true"><use :href="`#${icon}`"/></svg><span v-if="id === 'transfer-tasks'" class="transfer-task-count">{{ state.actions[id].count }}</span><span v-if="folded" class="menu-label">{{ label }}</span>
  </button>
</template>
