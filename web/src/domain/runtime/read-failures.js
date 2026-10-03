export function transientReadFailure(error) {
  if (!error) return false;
  if (error.name === 'AbortError' || error.name === 'TypeError') return true;
  const status = Number(error.status) || 0;
  return status === 408 || status === 429 || (status >= 500 && status !== 501);
}

/** 只有服务端可能已修复的失败才给“重试读取”：4xx 是请求本身不成立（不存在、
 *  超预算、格式错），重试同一读取不会有不同结果。 */
export function retryableReadFailure(failure) {
  const status = Number(failure?.status) || 0;
  return !status || status >= 500;
}

// 指数退避：1.5 s 起，每次翻倍，封顶 15 s；成功后归零。
export const RETRY_BASE_MS = 1500;
export const RETRY_MAX_MS = 15000;
export const retryDelay = attempt => Math.min(RETRY_MAX_MS, RETRY_BASE_MS * 2 ** Math.max(0, attempt));
