import {runtimePinia} from '../../stores/runtime/pinia'
import {createTypography} from '../../services/overlays/typography'
import { createApp } from 'vue'
import RecordsPage from './RecordsPage.vue'

createTypography().start()
createApp(RecordsPage).use(runtimePinia).mount(document.body)
