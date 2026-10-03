import {runtimePinia} from '../../stores/runtime/pinia'
import {createTypography} from '../../services/overlays/typography'
import { createApp } from 'vue'
import FilePage from './FilePage.vue'

createTypography().start()
createApp(FilePage).use(runtimePinia).mount(document.body)
