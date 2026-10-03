import { nextTick } from 'vue'
import type { useOverlaysStore } from '../../stores/overlays'
export interface FileDependencies {appUrl(path: string): string; allows(name: string): boolean; closeItemMenu(): void; alert(message: string): Promise<unknown>}
interface FileTarget {path?: string; href: string; kind?: string}
const isWindowsDrivePath = (path: string) => /^[A-Za-z]:[/\\]/.test(path)
function fileParentDirectory(path: string) {
 const slash = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
 return isWindowsDrivePath(path) && slash === 2 ? path.slice(0, 3) : path.slice(0, slash) || '/'
}
export const fileActions = [['copy-path', '复制完整路径'], ['copy-directory', '复制所在目录路径'], ['download', '下载'], ['copy-url', '复制链接地址'], ['open-web', '在新标签页打开']] as const
export async function copyFileText(text: string) {
 if (navigator.clipboard?.writeText) {try {await navigator.clipboard.writeText(text); return} catch { /* HTTP fallback */ }}
 const input = document.createElement('textarea')
 input.value = text; input.style.cssText = 'position:fixed;left:-10000px;top:0'
 document.body.appendChild(input); input.select()
 try {if (!document.execCommand('copy')) throw new Error('复制失败')}
 finally {input.remove()}
}
export function createFileMenu(state: ReturnType<typeof useOverlaysStore>, deps: FileDependencies) {
 let menu: HTMLElement
 let target: FileTarget | null = null
 function attach(element: HTMLElement) {menu = element}
 function close() {state.fileMenuOpen = false; target = null}
 async function contextmenu(event: MouseEvent) {
  const link = (event.target as Element).closest<HTMLAnchorElement>('.mb a[data-file-ref], .mb a[data-local-path], .mb a[data-reference-kind="web"]')
  if (!link) return
  event.preventDefault(); deps.closeItemMenu()
  const current = target = {path:link.dataset.localPath, href:link.dataset.fileHref || link.href, kind:link.dataset.referenceKind === 'web' ? 'web' : link.dataset.fileKind}
  const place = async () => {
   state.fileMenuOpen = true; await nextTick()
   const box = menu.getBoundingClientRect()
   state.fileMenu = {...state.fileMenu, left: Math.max(8, Math.min(event.clientX, innerWidth-box.width-8)), top: Math.max(8, Math.min(event.clientY, innerHeight-box.height-8))}
  }
  if (link.dataset.fileRef) {
   state.fileMenu = {...state.fileMenu, text:'正在读取文件信息…', actions:[]}; await place()
   try {
    if (!deps.allows('files')) throw new Error('Rust 后端尚未实现文件解析与文件操作。')
    const query = new URL(current.href).searchParams, ref = query.get('ref')
    const response = await fetch(deps.appUrl('/api/session/resolve-files'), {method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify({uid:query.get('uid'),agent:query.get('agent') || '',refs:[ref]})})
    const data = await response.json()
    if (!response.ok) throw new Error(data.error || '无法读取文件信息')
    const detail = data.targets?.find((item: {ref: string}) => item.ref === ref)
    const path = detail?.path || data.resolved?.[ref!]
    if (typeof path !== 'string' || !(path.startsWith('/') || path.startsWith('\\\\') || isWindowsDrivePath(path))) {
     const failure = data.errors?.find((item: {ref: string}) => item.ref === ref)
     throw new Error(failure?.error || '文件不存在或有多个同名文件，请使用完整路径。')
    }
    if (target !== current || !link.isConnected) return
    current.path = path; current.kind = ['file','directory'].includes(detail?.kind) ? detail.kind : 'unknown'
   } catch (error) {
    if (target === current) {state.fileMenu = {...state.fileMenu, text:(error as Error).message || '无法读取文件信息'}; await place()}
    return
   }
  }
  state.fileMenu = {...state.fileMenu, text:current.kind === 'web' ? current.href : current.path || '', label:current.kind === 'web' ? '链接操作' : '文件操作', actions:current.kind === 'web' ? ['copy-url','open-web'] : current.kind === 'file' ? ['copy-path','copy-directory','download'] : ['copy-path']}
  await place(); await nextTick(); menu.querySelector<HTMLButtonElement>('button:not([hidden])')!.focus({preventScroll:true})
 }
 function outside(event: Event) {if (!menu.contains(event.target as Node)) close()}
 function keydown(event: KeyboardEvent) {
  const buttons = [...menu.querySelectorAll<HTMLButtonElement>('button:not([hidden])')]
  const index = buttons.indexOf(document.activeElement as HTMLButtonElement)
  if (event.key === 'Escape') {event.preventDefault(); close()}
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
   event.preventDefault(); buttons[(index+(event.key === 'ArrowDown' ? 1 : buttons.length-1)) % buttons.length]!.focus()
  }
 }
 async function act(action: string) {
  const current = target
  if (!current) return
  close()
  try {
   if (action === 'copy-path') await copyFileText(current.path!)
   else if (action === 'copy-directory' && current.kind === 'file') await copyFileText(fileParentDirectory(current.path!))
   else if (action === 'copy-url') await copyFileText(current.href)
   else if (action === 'open-web') window.open(current.href,'_blank','noopener,noreferrer')
   else if (action === 'download') {
    const url = new URL(current.href); url.searchParams.set('download','1')
    const link = document.createElement('a'); link.href = url.href; link.download = ''
    document.body.appendChild(link); link.click(); link.remove()
   }
  } catch (error) {await deps.alert((error as Error).message || '文件操作失败')}
 }
 function start() {document.addEventListener('contextmenu',contextmenu); document.addEventListener('pointerdown',outside,true); addEventListener('resize',close); document.addEventListener('scroll',outside,true)}
 function dispose() {document.removeEventListener('contextmenu',contextmenu); document.removeEventListener('pointerdown',outside,true); removeEventListener('resize',close); document.removeEventListener('scroll',outside,true)}
 return {attach, close, keydown, act, start, dispose}
}
export type FileMenuController = ReturnType<typeof createFileMenu>
