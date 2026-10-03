export const fields = [
    ['cpu_cores', 'CPU', 'core'], ['gpu_count', 'GPU', 'gpu'],
    ['gpu_memory_bytes', '显存', 'bytes'], ['memory_pss_bytes', '内存 · PSS', 'bytes'],
    ['process_count', '进程数', 'count'],
    ['memory_bandwidth_bytes_per_second', '内存带宽', 'rate'],
    ['proc_storage_read_bytes_per_second', '存储层读取', 'rate'], ['proc_storage_write_bytes_per_second', '存储层写入', 'rate'],
    ['disk_read_operations_per_second', '本地读次数', 'ops'], ['disk_write_operations_per_second', '本地写次数', 'ops'],
    ['nfs_read_operations_per_second', 'NFS 读次数', 'ops'], ['nfs_write_operations_per_second', 'NFS 写次数', 'ops'],
    ['disk_read_bytes_per_second', '本地文件读取', 'rate'], ['disk_write_bytes_per_second', '本地文件写入', 'rate'],
    ['network_receive_bytes_per_second', 'TCP 接收', 'rate'], ['network_send_bytes_per_second', 'TCP 发送', 'rate'],
    ['nfs_read_bytes_per_second', 'NFS 读取', 'rate'], ['nfs_write_bytes_per_second', 'NFS 写入', 'rate'],
  ] as const;
export const descriptions: Record<string, string> = {
    disk_read_operations_per_second: '60 秒临时探测：本地普通文件成功读取次数，含缓存命中；不是硬盘物理 IOPS，不含内存映射、io_uring 和 splice。',
    disk_write_operations_per_second: '60 秒临时探测：本地普通文件成功写入次数；不是硬盘物理 IOPS，不含内存映射、io_uring 和 splice。',
    nfs_read_operations_per_second: '60 秒临时探测：NFS 文件成功读取次数，含缓存命中；不是远程 RPC 次数，不含内存映射、io_uring 和 splice。',
    nfs_write_operations_per_second: '60 秒临时探测：NFS 文件成功写入次数；不是远程 RPC 次数，不含内存映射、io_uring 和 splice。',
    process_count: '当前采样中已归属的进程数，按机器和进程身份去重。',
    cpu_cores: '占用的逻辑 CPU 核数；不同机器的核数不代表相同算力。',
    gpu_count: '使用到的计算设备数，同机按设备去重；不代表独占或满卡算力。约 10 秒更新。',
    gpu_memory_bytes: '计算进程的显存占用；不含纯图形任务，共享计算服务可能无法细分到工作进程。约 10 秒更新。',
    memory_bandwidth_bytes_per_second: '常驻硬件监控，约 5 秒更新；按会话统计总内存流量，未拆分读写。子会话依标签范围汇总；不代表逐进程带宽，短命任务可能漏计。',
    memory_pss_bytes: '按比例分摊共享内存后的驻留内存，不用普通驻留内存替代缺失值。约 30 秒更新。',
    proc_storage_read_bytes_per_second: '进程在存储层引起的读取量，与缓存命中的文件读取量不同。',
    proc_storage_write_bytes_per_second: '进程在存储层引起的写入量；延迟回写可能影响归属和时间。',
    disk_read_bytes_per_second: '本地普通文件的同步逻辑读取量，含缓存命中；不含 NFS、内存映射和异步直接通道。',
    disk_write_bytes_per_second: '本地普通文件的同步逻辑写入量，不等同于硬盘实际写入；不含 NFS、内存映射和异步直接通道。',
    network_receive_bytes_per_second: '应用收到的 TCP 数据，不含 UDP、RDMA、重传或 NFS 内核流量。',
    network_send_bytes_per_second: '应用发送的 TCP 数据，不含 UDP、RDMA、重传或 NFS 内核流量。',
    nfs_read_bytes_per_second: 'NFS 文件的同步逻辑读取量，含缓存命中；不是实际远程请求或网卡流量。',
    nfs_write_bytes_per_second: 'NFS 文件的同步逻辑写入量；不含完整远程请求、重传及协议开销。',
  };
export const reasons: Record<string, string> = {unsupported: '采集端不支持', unavailable: '暂无采样', offline: '机器离线', stale: '采样已过期', partial: '部分覆盖', warming_up: '等待下一次采样'};
export function formatResource(value: number, unit: string) {
 const format = (digits?: number) => value.toLocaleString('zh-CN', digits === undefined ? undefined : {maximumFractionDigits:digits})
 if (unit === 'count') return {text:format(), unit:' 个'}
 if (unit === 'ops') return {text:format(1), unit:' 次/s'}
 if (unit === 'core') return {text:format(2), unit:' 核'}
 if (unit === 'gpu') return {text:format(), unit:' 张'}
 const units = ['B','KiB','MiB','GiB','TiB']
 let i = 0
 while (value >= 1024 && i < units.length-1) {value /= 1024; i++}
 return {text:format(i ? 1 : 0), unit:` ${units[i]}${unit === 'rate' ? '/s' : ''}`}
}

export interface Metric {value?: number; status?: string; reason?: string; sampled_at?: number}
export type Metrics = Record<string, number | Metric>
export interface ResourceNode {node_id?: string; node_name?: string; status?: string; metrics?: Metrics; diagnostic?: {state: string; remaining_seconds?: number; error?: string}}
export interface ResourceData {nodes?: ResourceNode[]; sampled_at?: number; totals?: Metrics & {metrics?: Metrics}}
export interface ResourceSession {uid: string; title?: string; sid?: string}
export type ResourceScope = 'direct' | 'inclusive'
export const probeStates: Record<string, string> = {off:'未探测', starting:'正在启动', active:'探测中', stopping:'正在停止', failed:'探测失败', unsupported:'不支持探测'}
export function resourceMetrics(values?: Metrics, fallback?: string) {
 return fields.map(([key, label, unit]) => {
  const raw = values?.[key]
  const item: Metric = typeof raw === 'number' ? {value: raw, status: 'ok'} : (raw || {})
  const usable = Number.isFinite(item.value) && !['unsupported', 'unavailable', 'offline', 'stale', 'warming_up'].includes(item.status || '')
  const detail = /[\u3400-\u9fff]/.test(item.reason || '') ? item.reason : ''
  const status = reasons[item.status || ''] || (!usable ? reasons[fallback || ''] || '暂无采样' : '')
  const stamp = Number.isFinite(item.sampled_at) ? `采样 ${new Date(item.sampled_at! * 1000).toLocaleTimeString('zh-CN', {hour12:false})}` : ''
  const formatted = usable ? formatResource(item.value!, unit) : {text:'—', unit:''}
  return {key, label, partial: item.status === 'partial', tip: [descriptions[key], status, detail, stamp].filter(Boolean).join(' · '), text: formatted.text, unit: formatted.unit}
 })
}
