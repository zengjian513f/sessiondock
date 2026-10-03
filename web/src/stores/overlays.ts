import { defineStore } from 'pinia'
import { ref, shallowRef } from 'vue'
import type { ResourceData, ResourceScope } from '../domain/overlays/resources'
export const useOverlaysStore = defineStore('overlays', () => {
 const resourceData = shallowRef<ResourceData>()
 const resourceMessage = ref('')
 const resourceError = ref(false)
 const resourceSubtitle = ref('')
 const resourceScope = ref<ResourceScope>('inclusive')
 const probeState = ref('off')
 const probeError = ref('')
 const sleeping = ref(false)
 const fileMenu = shallowRef<{text: string; label: string; actions: string[]; left: number; top: number}>({text:'', label:'文件操作', actions:[], left:0, top:0})
 const fileMenuOpen = ref(false)
 return {resourceData, resourceMessage, resourceError, resourceSubtitle, resourceScope, probeState, probeError, sleeping, fileMenu, fileMenuOpen}
})
