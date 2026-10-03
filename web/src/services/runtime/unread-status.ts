export function createUnreadStatus(state: {unread: Map<string, any>},
  preferences: {set(key: string, value: unknown): void}, paint: (uid: string) => void) {
  function unreadRow(uid: string): {count: number} {
    const value = state.unread.get(uid)
    if (typeof value === 'number') return {count:value}
    return value && typeof value === 'object' ? {count:Math.max(0,+value.count || 0)} : {count:0}
  }
  function saveUnread() {
    preferences.set('unread',[...state.unread].filter(([,row]) => (+row?.count || +row || 0) > 0))
  }
  function addUnread(uid: string, count: number) {
    if (!uid || count <= 0) return
    const row = unreadRow(uid)
    state.unread.set(uid,{count:row.count+count})
    saveUnread()
    paint(uid)
  }
  function clearUnread(uid: string) {
    if (!uid || !state.unread.has(uid)) return
    state.unread.delete(uid)
    saveUnread()
    paint(uid)
  }
  return {unreadRow,saveUnread,addUnread,clearUnread}
}
