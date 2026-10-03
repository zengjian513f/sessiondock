import type { Pinia } from 'pinia'
import { useShellEnvironmentStore } from '../../stores/runtime/shell-environment'
import type { ShellEnvironmentItem } from '../../stores/runtime/shell-environment'
import type { NodeRecord } from '../../stores/runtime/nodes'

export interface ShellEnvironmentDependencies {
  hub: boolean
  nodes: {list: NodeRecord[]}
  hostname(): string
  fetch: typeof fetch
  appUrl(path: string): string
  audit(event: string, data: unknown): void
  alert(message: string): Promise<void>
}
export function createShellEnvironment(pinia: Pinia, deps: ShellEnvironmentDependencies) {
  const state = useShellEnvironmentStore(pinia)
  const ignored = new Map<string, string>()
  const restarting = new Map<string, {started_at: string | number | undefined; at: number}>()
  const restartWaitMs = 120000
  let run = 0
  function targets() {
    if (!deps.hub) return [{key:'local', name:deps.hostname() || deps.nodes.list[0]?.name || '本机', prefix:''}]
    return deps.nodes.list.filter(node => node.online !== false || restarting.has(node.id))
      .map(node => ({key:node.id, name:node.name || node.id, prefix:`api/nodes/${node.id}/`}))
  }
  async function check() {
    const current = ++run
    const found = await Promise.all(targets().map(async target => {
      let data: ShellEnvironmentItem['data'] | null = null
      try {
        const response = await deps.fetch(deps.appUrl(target.prefix + 'api/shell-env'), {cache:'no-store'})
        if (response.ok) data = await response.json()
      } catch { /* 机器重启或离线 */ }
      const restart = restarting.get(target.key)
      if (restart) {
        const back = data && data.started_at && data.started_at !== restart.started_at
        if (!back && Date.now() - restart.at < restartWaitMs) return {...target, data:data || {}, restarting:true}
        restarting.delete(target.key)
      }
      if (!data?.configured || !data.stale) return null
      return ignored.get(target.key) === (data.changed || []).join(',') ? null : {...target, data}
    }))
    if (current !== run) return
    state.revision++
    state.items = found.filter((item): item is ShellEnvironmentItem => item !== null)
  }
  function ignore(items: ShellEnvironmentItem[]) {
    for (const item of items) ignored.set(item.key, (item.data.changed || []).join(','))
    const keys = new Set(items.map(item => item.key))
    state.revision++
    state.items = state.items.filter(item => !keys.has(item.key))
  }
  async function restart(items: ShellEnvironmentItem[], button: HTMLButtonElement) {
    button.disabled = true
    const failed: string[] = []
    await Promise.all(items.map(async item => {
      try {
        const response = await deps.fetch(deps.appUrl(item.prefix + 'api/shell-env/restart'), {method:'POST'})
        const data = await response.json().catch(() => ({}))
        if (!response.ok || data.error) throw new Error(data.error || `HTTP ${response.status}`)
        restarting.set(item.key, {started_at:item.data.started_at, at:Date.now()})
        deps.audit('shell_env.restart', {node:item.key, changed:item.data.changed || []})
      } catch (error) {failed.push(`${item.name}: ${error instanceof Error ? error.message : String(error)}`)}
    }))
    state.revision++
    state.items = state.items.map(item => restarting.has(item.key) ? {...item, restarting:true} : item)
    if (failed.length) void deps.alert(`重启后端失败:\n${failed.join('\n')}`)
    for (let index = 0; index < 60 && items.some(item => restarting.has(item.key)); index++) {
      await new Promise(resolve => setTimeout(resolve, 2000))
      await check()
    }
  }
  function close() {ignore(state.items.filter(item => !item.restarting))}
  function start() {
    const timer = setInterval(check, 60000)
    const initial = setTimeout(check, 3000)
    const visible = () => {if (!document.hidden) void check()}
    document.addEventListener('visibilitychange', visible)
    return () => {clearInterval(timer);clearTimeout(initial);document.removeEventListener('visibilitychange',visible)}
  }
  return {state, check, ignore, restart, close, start}
}
export type ShellEnvironmentService = ReturnType<typeof createShellEnvironment>
