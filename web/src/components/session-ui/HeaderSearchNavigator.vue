<script setup lang="ts">
import {state} from '../../stores/search'
import {jumpMark} from '../../services/search/highlight'
defineProps<{inline:boolean}>()
</script><template><b id="mcount" :class="{capped:state.navigation.capped}" :title="state.navigation.title">{{state.navigation.text}}</b><button class="session-menu-action" id="m-prev" :role="inline?undefined:'menuitem'" title="上一处" aria-label="上一处匹配" @click="jumpMark(-1)">↑<span>上一处匹配</span></button><button class="session-menu-action" id="m-next" :role="inline?undefined:'menuitem'" title="下一处" aria-label="下一处匹配" @click="jumpMark(1)">↓<span>下一处匹配</span></button></template>
