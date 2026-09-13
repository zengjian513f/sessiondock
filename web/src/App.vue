<script setup lang="ts">
import BackendStatus from './components/BackendStatus.vue'
</script>

<template>
  <main class="workspace">
    <header>
      <p class="eyebrow">独立开发工作区</p>
      <h1>SessionDock</h1>
      <p class="intro">Rust 后端 · Vue 前端 · 独立 PTY host</p>
    </header>

    <BackendStatus />

    <section class="panel" aria-labelledby="scope-title">
      <h2 id="scope-title">从清晰的边界开始</h2>
      <p>这是可运行的目录骨架，尚未迁移原版 AgentHub 的业务功能。</p>
      <ul class="modules">
        <li><strong>Web</strong><span>Vue 3 / TypeScript / Pinia，按 API、状态和组件分层。</span></li>
        <li><strong>Server</strong><span>Axum / Tokio，目前只提供健康检查和静态资源。</span></li>
        <li><strong>ptyhost</strong><span>源码已导入；尚未接入浏览器终端或会话管理。</span></li>
      </ul>
    </section>

    <footer>未连接生产会话。旧前端快照仅作为迁移参考，不在此页面运行。</footer>
  </main>
</template>
