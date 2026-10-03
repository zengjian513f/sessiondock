<script setup lang="ts">
import type {SessionRow} from '../../domain/session-ui/types'
import InlineMarkup from './InlineMarkup'
import {onMounted,computed,nextTick,ref} from 'vue'
import {sessionUi as ui,appController as c} from '../../stores/session-ui'
import SessionAction from './SessionAction.vue'
import SessionMetadata from './SessionMetadata.vue'
const h=computed(()=>ui.header!)
const menuTarget=ref<HTMLElement|null>(null),metaTarget=ref<HTMLElement|null>(null),ready=ref(false)
onMounted(()=>{ready.value=true;nextTick(()=>c.value!.headerMounted())})
function consoleClick(event:Event){c.value!.consoleClick(event.currentTarget)}
function chainVisibility(row:SessionRow,event:Event){c.value!.chainVisibility(row,event.currentTarget)}
function menuClick(event:Event){if((event.target as Element).closest('button'))c.value!.closeSessionActions(true)}
</script><template><div class="dtitle">
<button class="mobile-back" title="返回会话列表" aria-label="返回会话列表" @click="c?.mobileBack()">←</button>
<h2 :class="{'has-session-views':h.hasAgents}"><InlineMarkup :html="h.icon"/><button v-if="h.hasAgents" class="session-view-switch" id="a-view-switch" type="button" title="切换主会话/子代理" aria-label="切换主会话/子代理" :aria-expanded="!!h.viewsOpen" @click="c?.toggleViews"><span>{{h.title}}</span><i>⌄</i></button><span v-else>{{h.title}}</span></h2>
<div v-if="h.hasAgents" class="session-view-menu" id="session-view-menu" :hidden="!h.viewsOpen" role="menu" @click.stop><button v-for="view in h.views" :key="view.id" type="button" :data-agent="view.id" :class="{on:view.on,running:view.running}" role="menuitem" @click="c?.selectView(view.id,$event)"><small><span class="view-kind"><template v-if="view.main">主会话</template><template v-else><i v-if="view.running" class="view-live" title="运行中" aria-label="运行中"></i>子代理 · {{view.type}}</template></span><span v-if="!view.main" class="view-span">{{view.span}}</span></small><b>{{view.title}}</b></button></div>
<div class="dbrief" :hidden="!h.brief"><Teleport v-for="(item,index) in h.metadata" :key="index" :to="metaTarget" :disabled="!ready || Number(index)<h.brief"><SessionMetadata :item="item" :index="Number(index)"/></Teleport></div>
<div class="dhead-actions" aria-label="会话操作">
<button v-if="c?.forkLabel" class="iconbtn" id="a-fork-chain" type="button" :title="c?.forkLabel" :aria-label="c?.forkLabel" aria-haspopup="menu" :aria-expanded="!!h.chainOpen" aria-controls="fork-chain-menu" @click="c?.toggleChain"><svg class="ui-icon" aria-hidden="true"><use href="#i-fork"/></svg></button>
<div v-if="c?.forkLabel" class="session-view-menu fork-chain-menu" id="fork-chain-menu" :hidden="!h.chainOpen" role="menu" :aria-label="c?.forkLabel" @click.stop><template v-for="(entry,index) in h.chain" :key="index"><div v-if="!entry.row" class="chain-row gone" role="none"><span><small>{{entry.level}} · 记录已不存在</small><b><code>{{entry.sid}}</code></b></span></div><div v-else class="chain-row" :class="{shown:!entry.row.fork_parent||entry.row.fork_parent_visible}" role="none" :data-uid="entry.row.uid"><button type="button" class="chain-open" role="menuitem" title="打开这条会话" @click="c?.chainOpen(entry.row)"><small>{{entry.level}} · {{c?.formatTime(entry.row.created)}} → {{c?.formatTime(entry.row.updated)}}{{!entry.row.fork_parent||entry.row.fork_parent_visible?' · 已在左栏':''}}</small><b>{{entry.row.title||entry.row.sid}}</b></button><button v-if="entry.row.fork_parent" type="button" class="btn chain-toggle" role="menuitem" :disabled="h.chainPending.includes(entry.row.uid)" :data-visible="entry.row.fork_parent_visible?0:1" @click="chainVisibility(entry.row,$event)">{{entry.row.fork_parent_visible?'隐藏':'显示'}}</button></div></template><div v-if="!h.chain.length" class="chain-row gone" role="none"><span><small>没有父会话或子会话</small></span></div></div>
<button class="iconbtn" id="a-term" type="button" title="打开控制台" aria-label="打开控制台" @mouseenter="c?.consoleHint()" @focus="c?.consoleHint()" @mouseleave="c?.clearConsoleHint()" @blur="c?.clearConsoleHint()" @click="consoleClick"><svg class="ui-icon" aria-hidden="true"><use href="#i-terminal"/></svg></button>
<Teleport v-for="(item,index) in h.items" :key="item.id||item.kind" :to="menuTarget" :disabled="!ready || Number(index)<h.inline"><SessionAction :item="item" :index="Number(index)" :inline="Number(index)<h.inline"/></Teleport>
<div class="session-actions" @focusout="c?.focusout"><button class="iconbtn" id="a-more" type="button" :title="h.moreLabel" :aria-label="h.moreLabel" aria-haspopup="menu" :aria-expanded="!!h.menuOpen" aria-controls="session-actions-menu" :hidden="h.moreHidden" @click="c?.toggleActions()" @keydown="c?.actionKey"><span aria-hidden="true">⋯</span></button><div id="session-actions-menu" class="session-actions-menu" :hidden="!h.menuOpen" @click="menuClick" @keydown="c?.menuKey"><div ref="menuTarget" role="menu" aria-label="会话操作"></div><div ref="metaTarget" class="dmeta"></div></div></div>
</div></div></template>
