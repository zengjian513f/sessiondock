export function fmtSize(n) {
  if (n < 1024) return n + 'B';
  if (n < 1048576) return (n / 1024).toFixed(0) + 'K';
  return (n / 1048576).toFixed(1) + 'M';
}
export function fmtTime(iso) {
  if (!iso) return '';
  const d = new Date(iso), now = new Date();
  const p = n => String(n).padStart(2, '0');
  const hm = `${p(d.getHours())}:${p(d.getMinutes())}`;
  if (d.toDateString() === now.toDateString()) return '今天 ' + hm;
  const y = new Date(now - 86400000);
  if (d.toDateString() === y.toDateString()) return '昨天 ' + hm;
  if (d.getFullYear() === now.getFullYear()) return `${d.getMonth() + 1}-${p(d.getDate())} ${hm}`;
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}
/** 起止时间：同一天只写一次日期前缀；没有结束时间就是还在跑。 */
export function fmtSpan(start, end) {
  const from = fmtTime(start);
  if (!end) return from ? `${from} → 运行中` : '运行中';
  const to = fmtTime(end);
  if (!from) return to;   // 旧节点的列表项还没有开始时间
  const split = s => { const i = s.lastIndexOf(' '); return i < 0 ? [s, ''] : [s.slice(0, i), s.slice(i + 1)]; };
  const [fromDay, fromClock] = split(from), [toDay, toClock] = split(to);
  return fromClock && toClock && fromDay === toDay ? `${from} → ${toClock}` : `${from} → ${to}`;
}
/** 家目录缩成 ~; 过长的路径中间省略, 首尾都是有信息量的部分。
 *  不能用 CSS direction:rtl 来截左边 —— bidi 会把开头的 "/" 挪到末尾。 */
export function shortCwd(p, max = 40) {
  p = (p || '(未知)').replace(/^\/home\/[^/]+/, '~');
  if (p.length <= max) return p;
  const seg = p.split('/');
  return seg.length > 3 ? `${seg[0]}/${seg[1]}/…/${seg.slice(-2).join('/')}` : p;
}
