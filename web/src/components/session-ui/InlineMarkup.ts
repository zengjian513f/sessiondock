import {defineComponent,h} from 'vue'
// Existing formatting helpers emit only small inline glyphs. Keep their actual
// sibling nodes and attributes, avoiding an extra wrapper in the original CSS.
export default defineComponent({props:{html:{type:String,required:true}},setup(props){return()=>{const doc=new DOMParser().parseFromString(props.html,'text/html');const node=(n:Node):any=>{if(n.nodeType===Node.TEXT_NODE)return n.textContent;const e=n as Element;return h(e.tagName.toLowerCase(),Object.fromEntries([...e.attributes].map(a=>[a.name,a.value])),[...e.childNodes].map(node))};return [...doc.body.childNodes].map(node)}}})
