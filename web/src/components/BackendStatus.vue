<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { useBackendStore } from '../stores/backend'

const backend = useBackendStore()
const label = computed(() => ({
  idle: '等待检查',
  checking: '正在检查后端…',
  online: 'Rust 后端已连接',
  offline: 'Rust 后端未连接',
}[backend.status]))

onMounted(() => { void backend.check() })
</script>

<template>
  <section class="panel" aria-labelledby="backend-title" :aria-busy="backend.status === 'checking'">
    <div class="panel-heading">
      <h2 id="backend-title">后端连接</h2>
      <button type="button" :disabled="backend.status === 'checking'" @click="backend.check()">
        {{ backend.status === 'checking' ? '检查中…' : '重新检查' }}
      </button>
    </div>
    <p class="connection" :data-state="backend.status" role="status">
      <span class="status-dot" aria-hidden="true"></span>{{ label }}
    </p>
    <p v-if="backend.health" class="muted">
      {{ backend.health.service }} · v{{ backend.health.version }} · 开发骨架
    </p>
    <div v-if="backend.error" class="error" role="alert">
      <p>{{ backend.error }}</p>
      <p>请从新仓库根目录运行 <code>cargo run -p sessiondock</code> 后重试。</p>
    </div>
  </section>
</template>
