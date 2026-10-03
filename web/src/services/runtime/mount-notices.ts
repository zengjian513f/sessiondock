import { h, render, watch } from 'vue'
import type { BuildService } from './build'
import type { ShellEnvironmentService } from './shell-environment'
import { StaleBuildNotice, LoginExpiredNotice } from './notices-view'
import { ShellEnvironmentNotice } from './shell-environment-view'

// Cards enter the existing stack when first shown, preserving chronological
// append order. A vanished environment card is removed and recreated on return.
export function mountRuntimeNotices(build: BuildService, environment: ShellEnvironmentService, stack: () => HTMLElement) {
  let stale: DocumentFragment | undefined, login: DocumentFragment | undefined, shell: DocumentFragment | undefined
  const append = (host: DocumentFragment) => {while (host.firstChild) stack().append(host.firstChild)}
  const watches = [
    watch(() => build.state.stale, value => {
      if (!value || stale) return
      stale = document.createDocumentFragment()
      render(h(StaleBuildNotice,{controller:build}),stale as unknown as HTMLElement)
      append(stale)
    },{immediate:true,flush:'sync'}),
    watch(() => build.state.loginExpired, value => {
      if (!value || login) return
      login = document.createDocumentFragment()
      render(h(LoginExpiredNotice,{controller:build}),login as unknown as HTMLElement)
      append(login)
    },{immediate:true,flush:'sync'}),
    watch(() => environment.state.items.length, value => {
      if (!value) {
        if (shell) render(null,shell as unknown as HTMLElement)
        shell = undefined
      } else if (!shell) {
        shell = document.createDocumentFragment()
        render(h(ShellEnvironmentNotice,{controller:environment}),shell as unknown as HTMLElement)
        append(shell)
      }
    },{immediate:true,flush:'sync'}),
  ]
  return () => {
    watches.forEach(stop => stop())
    for (const host of [stale,login,shell]) if (host) render(null,host as unknown as HTMLElement)
  }
}
