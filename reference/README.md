# 冻结前端参考

`legacy-web/` 是前身项目前端的静态快照，不是新 UI 的运行依赖。
不要将它直接接到生产 API，也不要为了构建新前端而修改这些文件。

生产前端是仓库根目录的 `legacy-web/`，以本快照为基线做小幅、能力门控的改动。
`tests/legacy_asset_diff.py` 与 `tests/legacy_text_diff.py` 报告两者的差异；
有意保留的基线改动记录在下面，`legacy_text_diff.py` 以此判定一处用户可见文本
差异是否已登记。

## 有意的基线改动

- 产品名与新写入的偏好命名空间是 SessionDock（`sessiondock.*` 键）；兼容的线上
  字段名、旧偏好键的回退读取和本快照保留既有客户端所需的历史标识。
- 附件引用分隔符按目标节点选择（`path_style`，旧节点按盘符/UNC 判断），Windows
  节点即使从 Linux/移动端浏览器上传也插入反斜杠路径；POSIX 引用保持 `./` 前缀。
  随之支持 composer 的原始附件端点；JSON 文件管理器上传完成端点仍受支持。
- 没有常驻的开发版横幅：SessionDock 是替代品而非开发版，`#backend-notice` 只在
  失败关闭的回退中出现。
