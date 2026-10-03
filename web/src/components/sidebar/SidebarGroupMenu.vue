<script setup lang="ts">
defineProps<{items: {name: string; checked: boolean}[]; busy: boolean; assign: (name: string) => void; keydown: (event: KeyboardEvent) => void}>()
</script>
<template>
  <button v-for="item in items" :key="item.name" type="button" :data-group-name="item.name" role="menuitemradio" :aria-checked="item.checked" :disabled="busy" @click="assign(item.name)" @keydown="keydown"><span class="group-menu-check" aria-hidden="true">{{ item.checked ? '✓' : '' }}</span>{{ item.name || '未分组' }}</button>
</template>
