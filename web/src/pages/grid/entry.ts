import {runtimePinia} from '../../stores/runtime/pinia'
import {createTypography} from '../../services/overlays/typography'
import { createApp } from 'vue'
import GridPage from './GridPage.vue'

createTypography().start()
createApp(GridPage).use(runtimePinia).mount(document.body)
