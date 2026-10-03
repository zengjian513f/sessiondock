import { defineComponent, h } from 'vue'
import type { PropType } from 'vue'
import type { BuildService } from './build'
export const StaleBuildNotice = defineComponent({
  props: {controller:{type:Object as PropType<BuildService>,required:true}},
  setup(props) {
    return () => {
      const {controller} = props, state = controller.state
      return h('div', {class:'app-float warn version-stale',role:'alert',hidden:state.staleHidden}, [
        h('div',{class:'app-float-head'},h('strong','SessionDock 已更新')),
        h('span',state.reloadMessage),
        h('div',{class:'app-float-actions'},[
          h('button',{class:'btn',type:'button',onClick:controller.later},'稍后'),
          h('button',{class:'btn primary','data-act':'reload',type:'button',
            title:state.serverBuild ? `服务器版本 ${state.serverBuild}` : '加载新版本',
            disabled:state.reloadBusy,onClick:controller.reload},state.reloadBusy ? '正在保存草稿…' : '重新加载'),
        ]),
      ])
    }
  },
})
export const LoginExpiredNotice = defineComponent({
  props: {controller:{type:Object as PropType<BuildService>,required:true}},
  setup(props) {
    return () => h('div',{class:'app-float warn login-expired',role:'alert'},[
      h('div',{class:'app-float-head'},h('strong','登录已失效')),
      h('span','自动同步已暂停。请在新窗口登录，再重新加载本页；未保存的输入仍保留在当前页面。'),
      h('div',{class:'app-float-actions'},h('button',{class:'btn primary',type:'button',onClick:props.controller.openLogin},'打开登录页')),
    ])
  },
})
