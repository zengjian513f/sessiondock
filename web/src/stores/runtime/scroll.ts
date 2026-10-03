import { defineStore } from 'pinia'
import { ref } from 'vue'
export const useConversationScrollStore = defineStore('runtime-conversation-scroll', () => {
  const stick = ref(true)
  const lastTop = ref(0)
  const lockUntil = ref(0)
  const selfScroll = ref(false)
  return {stick,lastTop,lockUntil,selfScroll}
})
