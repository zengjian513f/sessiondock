<script setup lang="ts">
import { state } from '../../stores/search'
</script>
<template><div id="stat" role="status" :class="{err: state.error}" :data-seq="state.sequence">{{ state.status }}</div></template>
