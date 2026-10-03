<script setup lang="ts">
import type { Chip } from '../../domain/sidebar/types'
import type { FilterGestures } from '../../services/sidebar/filters'
defineProps<{chips: Chip[]; nodes?: boolean; click: (key: string) => void; gestures: FilterGestures}>()
</script>
<template>
  <button v-for="chip in chips" :key="chip.key" type="button" :class="[nodes ? '' : 'chip', {on: chip.on, off: !nodes && !chip.on, 'node-offline': chip.offline, 'node-issue': chip.issue}]" :data-source="nodes ? undefined : chip.key" :data-node="nodes ? chip.key : undefined" :data-node-color="nodes ? chip.color : undefined" :data-label="nodes ? chip.label : undefined" :data-abbr="nodes ? chip.abbr : undefined" :data-count="nodes ? chip.count : undefined" :data-unavailable-reason="(!nodes || chip.offline) && chip.reason ? chip.reason : undefined" :aria-disabled="(!nodes || chip.offline) && chip.reason ? 'true' : undefined" :aria-description="(!nodes || chip.offline) && chip.reason ? chip.reason : undefined" :aria-pressed="chip.on" :title="chip.reason || chip.title" :aria-label="nodes ? `${chip.label} ${chip.count}${chip.reason ? `，${chip.reason}` : ''}` : `${chip.label}，${chip.count} 个会话`" @click="click(chip.key)" @click.capture="gestures.clickCapture" @contextmenu="gestures.contextmenu" @pointerdown="gestures.pointerdown" @pointerup="gestures.pointerup" @pointercancel="gestures.pointercancel" @pointerleave="gestures.pointerleave" @dblclick.capture="gestures.dblclick">
    <template v-if="nodes"><span class="node-name">{{ chip.label }}</span><span class="node-abbr">{{ chip.abbr }}</span><b class="node-count">{{ chip.count }}</b></template>
    <template v-else><svg class="ico source-icon" :data-source="chip.key" aria-hidden="true" :style="{color: chip.color}"><use :href="`#${chip.icon}`" /></svg><span>{{ chip.label }}</span><b>{{ chip.count }}</b></template>
  </button>
</template>
