<script setup lang="ts">
import type { MediaDiagnostic } from '../../services/overlays/media'
defineProps<{detail: MediaDiagnostic & {pending?: boolean}; notice: string; reloadBusy: boolean}>()
defineEmits<{retry: []; reload: []}>()
</script>
<template><span class="media-error media-load-error" role="status"><span>{{ notice }}</span><button v-if="!detail.pending" type="button" class="media-load-retry" @click="$emit('retry')">重试图片</button><template v-if="[404,409].includes(detail.status)"><span>图片凭据已失效或内容已变化；可手动重新载入当前会话获取新凭据。</span><button type="button" class="media-load-reload" :disabled="reloadBusy" @click="$emit('reload')">重新载入当前会话</button></template></span></template>
