<script setup lang="ts">
import ConsoleButton from './ConsoleButton.vue'
import {appController as c} from '../../stores/session-ui'
</script><template><div class="dhead"><div class="dtitle"><button class="mobile-back" type="button" title="返回会话列表" aria-label="返回会话列表" @click="c?.mobileBack()">←</button><h2>控制台</h2><div class="dhead-actions"><ConsoleButton/></div></div></div></template>
