<script setup lang="ts">
import { ref, computed, onBeforeUnmount } from 'vue'
import { operations } from '../../services/conversation/bridge'
import type { MediaView } from '../../services/overlays/media'
import MediaError from '../overlays/MediaError.vue'
const props = defineProps<{item: any}>()
const root = ref<HTMLElement>(), image = ref<HTMLImageElement>()
const lazy = computed(() => operations().lazyMediaEnabled() && props.item.lazy === true)
const src = computed(() => operations().safeMediaSrc(props.item.src))
const error = ref<any>(null), notice = ref(''), reloadBusy = ref(false)
let generation = 0
let failedView: MediaView | undefined
onBeforeUnmount(() => {generation++; failedView = undefined})
async function failed() {
  if (!lazy.value || !root.value?.isConnected) return
  if (image.value?.src !== new URL(src.value, location.href).href) return
  generation++
  const view = operations().createView(root.value, () => generation)
  if (!view.current()) return
  failedView = view
  notice.value = '图片加载失败；正在读取错误说明…'; error.value = {status:0, pending:true}
  const detail = await operations().diagnoseMedia(props.item.src)
  if (!view.current()) return
  error.value = detail
  notice.value = `图片不可用${detail.status ? `（HTTP ${detail.status}）` : ''}：${detail.message}`
}
function retry() {
  if (!failedView?.current()) return
  generation++; operations().forgetMediaDiagnostic(props.item.src); error.value = null; failedView = undefined
  if (image.value) image.value.src = src.value
}
async function reload() {
  const view = failedView
  if (!view?.current()) return
  reloadBusy.value = true
  try {await operations().reloadMediaOwned(view, (text: string) => notice.value = text)}
  finally {reloadBusy.value = false}
}
function loaded() {
  if (lazy.value && image.value && image.value.naturalWidth > 0) operations().forgetMediaDiagnostic(props.item.src)
}
</script>
<template>
  <span v-if="item.error" class="media-error" role="status">图片不可用：{{item.error.message || '图片不可用'}}</span>
  <span v-else-if="lazy" ref="root" class="media-load">
    <a class="media-link" :href="src" :hidden="!!error" target="_blank" rel="noopener noreferrer">
      <img ref="image" data-vue-media="true" loading="lazy" decoding="async" referrerpolicy="no-referrer" :src="src" :alt="item.alt || '图片'" :width="+item.width > 0 && Number.isFinite(+item.width) ? Math.round(+item.width) : undefined" :height="+item.height > 0 && Number.isFinite(+item.height) ? Math.round(+item.height) : undefined" data-media-lazy="true" :data-media-path="item.src" @error="failed" @load="loaded" />
      <span>{{item.alt || '图片'}}</span>
    </a>
    <MediaError v-if="error" :detail="error" :notice="notice" :reload-busy="reloadBusy" @retry="retry" @reload="reload" />
  </span>
  <a v-else class="media-link" :href="src" target="_blank" rel="noopener noreferrer">
    <img loading="lazy" decoding="async" referrerpolicy="no-referrer" :src="src" :alt="item.alt || '图片'" :width="+item.width > 0 && Number.isFinite(+item.width) ? Math.round(+item.width) : undefined" :height="+item.height > 0 && Number.isFinite(+item.height) ? Math.round(+item.height) : undefined" />
    <span>{{item.alt || '图片'}}</span>
  </a>
</template>
