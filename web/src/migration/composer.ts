import { h, render } from 'vue'
import Composer from '../components/composer/Composer.vue'
import { composerState, bindComposerController } from '../stores/composer'
// @ts-expect-error Extracted compatibility controller retains its JavaScript contracts.
import { createComposerController } from '../services/composer/controller.js'
let controller:any
export function createController(bridge:any) {
  controller=createComposerController(bridge,composerState)
  bindComposerController(controller)
  return controller
}
export function mount(host:HTMLElement) {
  // Keep #composer a direct #right child: existing terminal-first/full CSS uses
  // that relation. This fragment hosts Vue's vnode, never a legacy-built tree.
  const fragment=document.createDocumentFragment()
  render(h(Composer),fragment as unknown as HTMLElement)
  host.replaceWith(...fragment.childNodes)
}
export function operations():any {return controller}
