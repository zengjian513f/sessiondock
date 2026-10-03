import type { ResourceCell, Session } from '../../domain/sidebar/types'
const fields = [
  ['cpu_cores', 'resource-cpu', 'CPU 核数', 'core'], ['process_count', 'resource-process', '进程数', 'process'],
  ['memory_pss_bytes', 'resource-memory', '内存占用（PSS）', 'bytes'], ['gpu_count', 'resource-gpu', 'GPU 张数', 'gpu'],
  ['proc_storage_read_bytes_per_second', 'resource-read', '磁盘读取速度', 'rate'], ['proc_storage_write_bytes_per_second', 'resource-write', '磁盘写入速度', 'rate'],
] as const
const formats = [0, 1, 2].map(places => new Intl.NumberFormat('zh-CN', {maximumFractionDigits: places, useGrouping: false}))
const digits = (n: number, places = 1) => formats[places]!.format(n)
export function compact(value: number, unit: string): string {
  if (unit === 'core') return value > 0 && value < .01 ? '<0.01' : digits(value, 2)
  if (unit === 'process' || unit === 'gpu') return digits(value, 0)
  if (!value) return '0'
  const units = ['B', 'K', 'M', 'G', 'T', 'P']; let i = 0
  while (value >= 1024 && i < units.length - 1) {value /= 1024; i++}
  return digits(value, value < 10 && i ? 1 : 0) + units[i] + (unit === 'rate' ? '/s' : '')
}
export interface Metric {value: number | null; status: string}
export function cells(session: Session, agent: boolean, rows: Map<string, Record<string, Metric>>, localNode: string, sampledAt: number): ResourceCell[] {
  const age = Date.now() / 1000 - sampledAt
  const metrics = age >= -5 && age <= 15 && !agent ? rows.get(JSON.stringify([session.node_id || localNode, session.source, session.sid])) : undefined
  return fields.map(([field, icon, label, unit]) => {
    const metric = metrics?.[field]
    const valid = metric && Number.isFinite(metric.value) && metric.value! >= 0 && ['ok', 'partial'].includes(metric.status)
    return {field, icon, label, value: valid ? compact(metric.value!, unit) : '—'}
  })
}
