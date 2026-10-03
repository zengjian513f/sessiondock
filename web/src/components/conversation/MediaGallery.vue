<script setup lang="ts">
import { computed } from 'vue'
import { operations } from '../../services/conversation/bridge'
import MediaImage from './MediaImage.vue'
const props = defineProps<{items?: any[]; more?: any}>()
const visible = computed(() => (props.items || []).filter(x => x.gallery || !x.ref).filter(x => x.error || operations().safeMediaSrc(x.src)))
const info = computed(() => operations().mediaContinuationEnabled() ? operations().mediaMoreInfo(props.more) : null)
</script>
<template>
  <div v-if="visible.length || info" class="media-gallery">
    <MediaImage v-for="(item, index) in visible" :key="item.src || index" :item="item" />
    <span v-if="operations().mediaError(info?.cursor)" class="media-page-error" role="alert">{{operations().mediaError(info?.cursor)}}</span>
    <button v-if="info" type="button" class="media-more" :data-media-cursor="info.cursor" :disabled="!info.cursor || operations().mediaBusy(info.cursor)" :title="`共 ${info.total.toLocaleString()} 张图片`" @click="operations().loadMedia(info.cursor, $event.currentTarget)">
      {{operations().mediaBusy(info.cursor) ? '正在读取图片…' : operations().mediaError(info.cursor) ? '重试加载图片' : `还有 ${info.remaining.toLocaleString()} 张图片${info.cursor ? '，加载下一批' : '暂不可加载'}`}}
    </button>
  </div>
</template>
