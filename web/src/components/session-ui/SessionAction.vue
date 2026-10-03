<script setup lang="ts">
import type {HeaderAction} from '../../domain/session-ui/types'
import {computed} from 'vue'
import InlineMarkup from './InlineMarkup'
import HeaderSearchNavigator from './HeaderSearchNavigator.vue'
import {sessionUi as ui,appController as c} from '../../stores/session-ui'
const header=computed(()=>ui.header!)
defineProps<{item:HeaderAction,index:number,inline:boolean}>()
function activate(item:HeaderAction){
 if(item.id==='a-star')c.value!.toggleStar();
 else if(item.id==='a-turns')c.value!.toggleTurns();
 else if(item.id==='a-clone-group')c.value!.cloneGroup();
 else if(item.id==='a-global-new-session')c.value!.newSessionProxy();
 else if(item.id==='a-global-settings')c.value!.settingsProxy();
 else if(item.id==='a-global-page-reload')c.value!.reloadProxy();
 else if(item.id==='a-global-transfer-tasks')c.value!.transfersProxy();
}
function freeze(event:Event){c.value!.freezeSession(event.currentTarget)}
function sessionAction(event:Event){c.value!.sessionAction(event.currentTarget)}

</script><template>
<div v-if="item.kind==='search'" class="session-menu-search" data-search-navigator :data-order="index" :data-from-menu="inline?'1':undefined"><HeaderSearchNavigator :inline="inline"/></div>
<div v-else-if="item.kind==='diagnostics'" class="session-menu-diagnostics" :data-order="index" :data-from-menu="inline?'1':undefined">
<button v-if="!header.meta.agent_id && !header.pending" class="session-menu-action" id="a-session-freeze" :disabled="header.freeze.disabled" :hidden="header.freeze.hidden" :title="header.freeze.reason || header.freeze.label" :aria-label="header.freeze.label" :aria-pressed="header.freeze.pressed" :data-unavailable-reason="header.freeze.reason||undefined" :aria-disabled="header.freeze.reason?'true':undefined" :aria-description="header.freeze.reason||undefined" :role="inline?undefined:'menuitem'" @click="freeze"><svg class="ui-icon" aria-hidden="true"><use :href="`#i-${header.freeze.icon}`"/></svg><span>{{header.freeze.label}}</span></button>
<button class="session-menu-action" data-report-bug title="报告当前会话问题" aria-label="报告当前会话问题" :role="inline?undefined:'menuitem'"><svg class="ui-icon" aria-hidden="true"><use href="#i-bug"/></svg><span>报告当前会话问题</span></button>
</div>
<button v-else-if="item.kind==='action'" :disabled="header.action.disabled" :class="header.action.class" id="a-session-action" :title="header.action.label" :aria-label="header.action.label" :data-order="index" :data-from-menu="inline?'1':undefined" :role="inline?undefined:'menuitem'" @click="sessionAction"><svg class="ui-icon" aria-hidden="true"><use :href="`#i-${header.action.icon}`"/></svg><span>{{header.action.label}}</span></button>
<button v-else-if="item.available!==false" type="button" :id="item.id" :class="item.class" :title="item.reason||item.label" :aria-label="item.label" :data-unavailable-reason="item.reason||undefined" :aria-disabled="item.reason?'true':undefined" :aria-description="item.reason||undefined" :aria-pressed="item.pressed" :data-star-uid="item.starUid" :disabled="item.disabled" :data-order="index" :data-from-menu="inline?'1':undefined" :role="inline?undefined:'menuitem'" @click="activate(item)"><InlineMarkup v-if="item.iconHtml" :html="item.iconHtml"/><svg v-else class="ui-icon" aria-hidden="true"><use :href="`#i-${item.icon}`"/></svg><span>{{item.label}}</span></button>
</template>
