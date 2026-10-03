<script lang="ts">
import { defineComponent, h, createStaticVNode, getCurrentInstance, onMounted, onUpdated } from 'vue'
import { operations } from '../../services/conversation/bridge'

// Static HTML is limited to established Markdown inner content. Fragment mode
// preserves the original direct .mb children without adding a layout wrapper.
// Cards and controls remain ordinary Vue templates.
export default defineComponent({
  inheritAttrs:false,
  props:{html:String,text:String,tag:{type:String,default:'div'},syntax:{type:Boolean,default:true},diff:Boolean,fragment:Boolean},
  setup(props,{attrs}) {
    const instance=getCurrentInstance()!
    let painted:string | undefined
    let staticHtml:string | undefined, staticVersion=0
    function paint() {
      const version=props.html ?? props.text ?? ''
      if(painted===version)return
      painted=version
      const vnode=instance.subTree
      let node=vnode.el as Node | null
      const end=vnode.anchor || node
      while(node){
        if(node instanceof HTMLElement){
          node.dataset.conversationInner=''
          if(props.html===undefined)operations().clearSyntaxPaint(node)
          if(props.diff)operations().paintToolOutputDiff(node)
          if(props.html!==undefined)operations().renderFormulae(node)
          if(props.syntax)operations().paintSyntax(node)
          operations().markInner(node)
        }
        if(node===end)break
        node=node.nextSibling
      }
    }
    onMounted(paint);onUpdated(paint)
    return ()=>{
      if(props.fragment){
        const html=props.html || '<!--empty markdown-->'
        const template=document.createElement('template');template.innerHTML=html
        if(html!==staticHtml){staticHtml=html;staticVersion++}
        const vnode=createStaticVNode(html,template.content.childNodes.length)
        vnode.key=staticVersion
        return vnode
      }
      return h(props.tag,{...attrs,'data-conversation-inner':'',...(props.html!==undefined ? {innerHTML:props.html} : {})},props.html===undefined ? props.text || '' : undefined)
    }
  },
})
</script>
