import { defineComponent, h } from 'vue'
import type { PropType } from 'vue'
import type { ShellEnvironmentService } from './shell-environment'

// Preserve the existing floating-card element order, classes and labels.
export const ShellEnvironmentNotice = defineComponent({
  props: {controller:{type:Object as PropType<ShellEnvironmentService>, required:true}},
  setup(props) {
    const button = (label:string, aria:string, action:(event:MouseEvent)=>void, options:Record<string,unknown> = {}) =>
      h('button', {class:'btn', type:'button', ...(aria ? {'aria-label':aria} : {}), onClick:action, ...options}, label)
    return () => {
      const {controller} = props, items = controller.state.items
      if (!items.length) return null
      const pending = items.filter(item => !item.restarting)
      return h('div', {id:'shell-env-notice',class:'app-float warn shell-env-stale',role:'status'}, [
        h('div', {class:'app-float-head',key:`head:${controller.state.revision}`}, [
          h('strong', '登录环境（zshrc）已变化'),
          ...(pending.length > 1 ? [button(`全部重启 (${pending.length})`, '', event => void controller.restart(pending,event.currentTarget as HTMLButtonElement))] : []),
          button('×', '关闭', () => controller.close(), {class:'modal-close', title:'忽略这些变化'}),
        ]),
        h('span', '新开的会话仍用旧环境，重启后端后生效；正在运行的会话不受影响。'),
        h('table', {class:'shell-env-table',key:`table:${controller.state.revision}`}, [
          h('thead', h('tr', ['机器','变化','',''].map(title => h('th', title)))),
          h('tbody', items.map(item => h('tr', {'data-node':item.key,key:item.key}, [
            h('td', {class:'shell-env-node'}, item.name),
            h('td', {class:'shell-env-changed'}, item.restarting ? '正在重启…' : (item.data.changed || []).join('、')),
            h('td', item.restarting ? [] : [button('重启', `重启 ${item.name} 后端`, event => void controller.restart([item], event.currentTarget as HTMLButtonElement))]),
            h('td', item.restarting ? [] : [button('忽略', `忽略 ${item.name}`, () => controller.ignore([item]))]),
          ]))),
        ]),
      ])
    }
  },
})
