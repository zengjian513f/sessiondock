<script setup lang="ts">
import { ref, computed, onBeforeUnmount } from 'vue'
import { operations } from '../../services/conversation/bridge'
const props = defineProps<{item: any}>()
const root = ref<HTMLElement>(), image = ref<HTMLImageElement>()
const lazy = computed(() => operations().lazyMediaEnabled() && props.item.lazy === true)
const src = computed(() => operations().safeMediaSrc(props.item.src))
const error = ref<any>(null), notice = ref(''), reloadBusy = ref(false)
let generation = 0
onBeforeUnmount(() => generation++)
async function failed() {
  if (!lazy.value || !root.value?.isConnected) return
  const current = ++generation
  notice.value = '图片加载失败；正在读取错误说明…'; error.value = {status:0, pending:true}
  const detail = await operations().diagnoseMedia(props.item.src)
  if (generation !== current || !root.value?.isConnected) return
  error.value = detail
  notice.value = `图片不可用${detail.status ? `（HTTP ${detail.status}）` : ''}：${detail.message}`
}
function retry() {
  generation++; operations().forgetMediaDiagnostic(props.item.src); error.value = null
  if (image.value) image.value.src = src.value
}
async function reload() {
  reloadBusy.value = true
  const element = root.value!
  const current = generation
  await operations().reloadMediaOwned({
    current: () => current === generation && element.isConnected,
  }, (text: string) => notice.value = text)
  reloadBusy.value = false
}
</script>
<template>
  <span v-if="item.error" class="media-error" role="status">图片不可用：{{item.error.message || '图片不可用'}}</span>
  <span v-else-if="lazy" ref="root" class="media-load">
    <a class="media-link" :href="src" :hidden="!!error" target="_blank" rel="noopener noreferrer">
      <img ref="image" data-vue-media="true" loading="lazy" decoding="async" referrerpolicy="no-referrer" :src="src" :alt="item.alt || '图片'" :width="+item.width > 0 && Number.isFinite(+item.width) ? Math.round(+item.width) : undefined" :height="+item.height > 0 && Number.isFinite(+item.height) ? Math.round(+item.height) : undefined" data-media-lazy="true" :data-media-path="item.src" @error="failed" @load="operations().forgetMediaDiagnostic(item.src)" />
      <span>{{item.alt || '图片'}}</span>
    </a>
    <span v-if="error" class="media-error media-load-error" role="status">
      <span>{{notice}}</span>
      <button v-if="!error.pending" type="button" class="media-load-retry" @click="retry">重试图片</button>
      <template v-if="[404,409].includes(error.status)">
        <span>图片凭据已失效或内容已变化；可手动重新载入当前会话获取新凭据。</span>
        <button type="button" class="media-load-reload" :disabled="reloadBusy" @click="reload">重新载入当前会话</button>
      </template>
    </span>
  </span>
  <a v-else class="media-link" :href="src" target="_blank" rel="noopener noreferrer">
    <img loading="lazy" decoding="async" referrerpolicy="no-referrer" :src="src" :alt="item.alt || '图片'" :width="+item.width > 0 && Number.isFinite(+item.width) ? Math.round(+item.width) : undefined" :height="+item.height > 0 && Number.isFinite(+item.height) ? Math.round(+item.height) : undefined" />
    <span>{{item.alt || '图片'}}</span>
  </a>
</template>
