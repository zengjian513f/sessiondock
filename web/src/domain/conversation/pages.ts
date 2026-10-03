// @ts-nocheck
// Existing wire validation; no new admission or capability policies.
const HISTORY_PAGE_MAX_EVENTS=10000;
let mediaMoreInfo;
export function configurePages(value){mediaMoreInfo=value;}
export function validateHistoryPage(data, partial, cursor) {
  const page = data?.page;
  const integer = value => Number.isSafeInteger(value) && value >= 0;
  const token = value => typeof value === 'string' && /^[0-9a-f]{32}$/.test(value);
  if (!Array.isArray(data?.messages) || data.messages.length > HISTORY_PAGE_MAX_EVENTS || !page || !token(cursor) || page.cursor !== cursor
      || !data.messages.every(message => message && typeof message === 'object' && !Array.isArray(message)
        && typeof message.role === 'string' && typeof message.text === 'string'
        && (message.media == null || (Array.isArray(message.media)
          && message.media.every(item => item && typeof item === 'object' && !Array.isArray(item)))))
      || !integer(partial.head) || !integer(partial.omitted)
      || !['start', 'end', 'stop', 'remaining'].every(key => integer(page[key]))
      || page.start !== partial.head || page.stop !== partial.head + partial.omitted
      || page.end < page.start || page.end > page.stop
      || page.end - page.start !== data.messages.length || !data.messages.length
      || page.remaining !== page.stop - page.end
      || (page.remaining === 0 ? page.next !== null : !token(page.next) || page.next === cursor)) {
    throw new Error('历史分页响应与当前缺口不匹配；请重新载入当前历史。');
  }
  return page;
}

export function validateMediaPage(data, more, cursor) {
  const page = data?.page, info = mediaMoreInfo(more);
  const integer = value => Number.isSafeInteger(value) && value >= 0;
  const token = value => typeof value === 'string' && /^[0-9a-f]{32}$/.test(value);
  if (!Array.isArray(data?.media) || data.media.length > 16 || !data.media.length || !page || !info
      || !token(cursor) || page.cursor !== cursor
      || !data.media.every(item => item && typeof item === 'object' && !Array.isArray(item))
      || !['start', 'end', 'total', 'remaining'].every(key => integer(page[key]))
      || page.total !== info.total || page.start !== info.total - info.remaining
      || page.end <= page.start || page.end > page.total
      || page.end - page.start !== data.media.length
      || page.remaining !== page.total - page.end
      || (page.remaining === 0 ? page.next !== null : !token(page.next) || page.next === cursor)) {
    throw new Error('图片分页响应与当前消息不匹配；请重新载入当前会话。');
  }
  return page;
}


export function insertHistoryPage(entry,data,page,bytes){
  entry.msgs=[...entry.msgs.slice(0,page.start),...data.messages,...entry.msgs.slice(page.start)];
  entry.partial=page.remaining ? {...entry.partial,head:page.end,omitted:page.remaining,cursor:page.next} : null;
  entry.bytes=(entry.bytes || 0)+bytes;
}
