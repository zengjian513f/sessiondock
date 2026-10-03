export const STOP_STAGE_TEXT = {
  graceful: 'CLI 已在收到 Ctrl-D 后退出',
  stopped: 'CLI 未响应 Ctrl-D，已由宿主停止并确认退出',
  already_exited: '该受管实例此前已退出',
  uncertain: '已发送 Ctrl-D 与宿主停止指令，但限时内未观察到退出；结果不确定，不会自动重试',
}
