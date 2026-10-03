<script setup lang="ts">
import {ref,onMounted} from 'vue'
import {answer} from '../../services/session-ui/popup'
const props=defineProps<{spec:any}>();const dialog=ref<HTMLDialogElement>();
function finish(value:unknown){answer(props.spec,value,dialog.value!)}
function cancel(){finish((props.spec.buttons.find((b:any)=>b.action==='cancel')||props.spec.buttons[0]).value)}
onMounted(()=>{dialog.value!.showModal();const action=props.spec.buttons.find((b:any)=>b.primary)||props.spec.buttons.at(-1);dialog.value!.querySelector<HTMLButtonElement>(`[data-popup-action="${action.action}"]`)?.focus()})
</script><template><dialog ref="dialog" class="app-dialog app-popup" :data-popup-type="spec?.type" :data-message="spec?.message" :aria-labelledby="spec?.id" @cancel.prevent="cancel"><div class="app-popup-panel"><div class="modal-head"><div><h2 :id="spec?.id">{{spec?.title}}</h2></div><button type="button" class="modal-close" title="关闭" aria-label="关闭" @click="cancel">×</button></div><div v-if="spec?.body" class="app-popup-message">{{spec?.body}}</div><div class="modal-actions"><button v-for="button in spec?.buttons" :key="button.action" type="button" :class="button.className||'btn'" :data-popup-action="button.action" @click="finish(button.value)">{{button.label}}</button></div></div></dialog></template>
