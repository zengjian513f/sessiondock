import type { MachineClient, MachineTarget } from '../services/machines'

export function compareVersions(a: string, b: string): number {
  const parts = (v: string) => String(v).split('-')[0]!.split('.').map(n => parseInt(n, 10));
  const [x, y] = [parts(a), parts(b)];
  for (let i = 0; i < Math.max(x.length, y.length); i++) {
    const d = (x[i] || 0) - (y[i] || 0);
    if (Number.isNaN(d)) return 0;
    if (d) return Math.sign(d);
  }
  return String(a).includes('-') === String(b).includes('-') ? 0 : String(a).includes('-') ? -1 : 1;
}
export function clientUpdateSummary(target: MachineTarget, client: MachineClient, clientName: string): string {
  const update = client.update || {};
  const name = `${target.name}：${clientName}`;
  if (update.ok) {
    return update.before && update.after && update.before !== update.after
      ? `${name} 已更新 ${update.before} → ${update.after}。`
      : `${name} 已是最新版本${update.after ? ` ${update.after}` : ''}。`;
  }
  const last = String(update.output || '').trim().split('\n').pop() || '';
  const code = update.code == null ? '' : `（退出码 ${update.code}）`;
  return `${name} 更新失败${code}${last ? `：${last}` : '。'}`;
}
export function clientCell(client: MachineClient, reference: string | undefined, published: boolean, summary: string) {
  const running = !!client.update?.running;
  const outdated = client.version && reference && compareVersions(client.version, reference) < 0;
  const state = running ? 'running' : outdated ? 'outdated' : client.version && published ? 'current' : 'unknown';
  const lookup = client.latest_state === 'pending' ? '正在查询最新版本'
    : client.latest_state === 'failed' ? '最新版本查询失败' : '';
  const tips = [client.detail || '', outdated ? `可更新到 ${reference}`
    : state === 'current' ? `已是最新（${reference}）` : '未查到最新版本', lookup];
  if (client.update && !running) {
    tips.push(summary);
    if (client.update.output) tips.push(client.update.output);
  }
  return {state, title: tips.filter(Boolean).join('\n\n'), running};
}
