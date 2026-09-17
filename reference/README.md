# 冻结前端参考

`legacy-web/` 是前身项目前端的静态快照，不是新 UI 的运行依赖。
不要将它直接接到生产 API，也不要为了构建新前端而修改这些文件。

生产前端是仓库根目录的 `legacy-web/`，以本快照为基线做小幅、能力门控的改动。
`tests/legacy_asset_diff.py` 与 `tests/legacy_text_diff.py` 报告两者的差异；
有意保留的基线改动记录在下面，`legacy_text_diff.py` 以此判定一处用户可见文本
差异是否已登记。

## 有意的基线改动

- 报告问题弹窗沿用会话输入栏：附件卡片在输入框上方，加号、自动增高的输入框和发送键
  底边对齐；只保留机器与 CLI 选择、问题描述、附件和关闭入口，移除长说明与取消按钮。
- 控制台打开期间的重复点击使用非阻塞等待提示；控制权 HTTP 请求（含响应正文）
  在 5 秒后中止本次等待，提示服务端可能已取得控制权，清理本页忙碌状态。
  不自动重试或强制抢占，也不改变输入发送行为。超时与忙碌提示不弹原生对话框。
- 产品名与新写入的偏好命名空间是 SessionDock（`sessiondock.*` 键）；兼容的线上
  字段名、旧偏好键的回退读取和本快照保留既有客户端所需的历史标识。
- 附件引用分隔符按目标节点选择（`path_style`，旧节点按盘符/UNC 判断），Windows
  节点即使从 Linux/移动端浏览器上传也插入反斜杠路径；POSIX 引用保持 `./` 前缀。
  随之支持 composer 的原始附件端点；JSON 文件管理器上传完成端点仍受支持。
- 没有常驻的开发版横幅：SessionDock 是替代品而非开发版，`#backend-notice` 只在
  失败关闭的回退中出现。

- Reports and conversations share browser draft persistence and saved-input recovery; new sessions enter conversation mode while their CLI starts on the backend.
- 未落盘的新建会话顶栏是删除会话（等同丢弃），不是关机停止；已落盘且仍在运行的才是停止。
- 已完成回合折叠时，主助手的第一条原生 final（Claude `end_turn`、Codex `final_answer`）就是露在顶层的结论；Stop hook 拒绝收尾后追加的工具调用与短补充、后台 task 短报单独成段规划，不再把长结论折进过程合集只露出末尾几行。
