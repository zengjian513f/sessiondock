<script setup lang="ts">
import {appController as c} from '../../stores/session-ui'
function click(e:Event){c.value!.consoleClick(e.currentTarget)}
</script><template><div class="dhead"><div class="dtitle"><button class="mobile-back" type="button" title="返回会话列表" aria-label="返回会话列表" @click="c?.mobileBack()">←</button><h2>控制台</h2><div class="dhead-actions"><button class="iconbtn" id="a-term" type="button" title="打开控制台" aria-label="打开控制台" @mouseenter="c?.consoleHint()" @focus="c?.consoleHint()" @mouseleave="c?.clearConsoleHint()" @blur="c?.clearConsoleHint()" @click="click"><svg class="ui-icon" aria-hidden="true"><use href="#i-terminal"/></svg></button></div></div></div></template>
