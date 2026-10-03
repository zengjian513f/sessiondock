import { defineStore } from 'pinia'
import { shallowRef,ref } from 'vue'

export interface NodeRecord {
  id: string
  name: string
  color?: string
  online?: boolean
  [field: string]: any
}
export const useNodeStore = defineStore('runtime-nodes', () => {
  const visible=ref(false)
  const list = shallowRef<NodeRecord[]>([])
  const machines = shallowRef<NodeRecord[]>([])
  const off = shallowRef(new Set<string>())
  const capabilities = shallowRef<Record<string, any>>({})
  const errors = shallowRef(new Map<string, any[]>())
  return { visible,list, machines, off, capabilities, errors }
})
export const useConsoleStore = defineStore('runtime-console', () => {
  const errors = shallowRef(new Map<string, string>())
  const busy = shallowRef(new Set<string>())
  return { errors, busy }
})
