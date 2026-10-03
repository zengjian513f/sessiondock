import type { SearchNode } from './types'
export function nodeStatus(node: SearchNode) {
 const count = node.total !== null ? `${node.done} / ${node.total}` : `已扫描 ${node.done}`
 const labels: Record<string, string> = {preparing: '准备中', offline: '离线跳过', error: '搜索失败', limited: `${count} · 达到结果上限`, done: `${count} · 已完成`, scanning: `${count} 个会话`}
 return labels[node.state] || count
}
