import type { Pinia } from 'pinia'
import type { NetworkService } from './network.js'
import { useRuntimeNoticesStore } from '../../stores/runtime/notices'
export interface BuildDependencies {
  buildId: string
  appBase: URL
  network: NetworkService
  fetch: typeof fetch
  appUrl(path: string): string
  audit(event: string, data: unknown): void
  prepareComposerReload?: () => Promise<boolean>
  disableSending(): void
}
export function createBuildService(pinia: Pinia, deps: BuildDependencies) {
  const state = useRuntimeNoticesStore(pinia)
  function markStaleBuild(serverBuild = '') {
    if (state.stale) {
      if (state.staleHidden && !document.hidden) state.staleHidden = false
      return
    }
    state.stale = true
    state.serverBuild = serverBuild
    deps.network.pause('stale')
    document.body.classList.add('stale-build')
    deps.audit('build.stale', {server_build:serverBuild})
    deps.disableSending()
  }
  async function checkServerBuild() {
    if (deps.network.paused) return
    try {
      const response = await deps.fetch(deps.appUrl('api/meta'), {cache:'no-store'})
      const data = await response.json()
      if (data.hostname) state.hostname = data.hostname
      if (data.build && deps.buildId && data.build !== deps.buildId) markStaleBuild(data.build)
    } catch { /* 网络恢复后再检查 */ }
  }
  async function reload() {
    state.reloadBusy = true
    try {
      if (deps.prepareComposerReload && !await deps.prepareComposerReload()) {
        state.reloadMessage = '草稿尚未保存或附件尚未上传完成，已取消重新加载。请等待保存成功或保留内容后重试。'
        return
      }
      location.reload()
    } finally {state.reloadBusy = false}
  }
  function showLoginExpired() {state.loginExpired = true}
  function openLogin() {window.open(deps.appBase, '_blank', 'noopener')}
  function start() {
    const timer = setInterval(checkServerBuild, 30000)
    queueMicrotask(checkServerBuild)
    const visible = () => {if (!document.hidden) void checkServerBuild()}
    document.addEventListener('visibilitychange', visible)
    return () => {clearInterval(timer);document.removeEventListener('visibilitychange',visible)}
  }
  return {state, markStaleBuild, checkServerBuild, reload, showLoginExpired, openLogin, start,
    later() {state.staleHidden = true}}
}
export type BuildService = ReturnType<typeof createBuildService>
