<script setup lang="ts">
import { resourceMetrics } from '../../domain/overlays/resources'
import type { Metrics } from '../../domain/overlays/resources'
defineProps<{values?: Metrics; fallback?: string}>()
</script>
<template><div v-for="metric in resourceMetrics(values, fallback)" :key="metric.key" class="sr-metric" :class="{'sr-partial': metric.partial}" tabindex="0" :title="metric.tip"><dt>{{ metric.label }}</dt><dd>{{ metric.text }}<small v-if="metric.unit">{{ metric.unit }}</small></dd></div></template>
