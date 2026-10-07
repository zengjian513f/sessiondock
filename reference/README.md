# 冻结前端参考

本目录的 `legacy-web/` 是前身项目前端的冻结静态快照，仅用于历史参考。
不要将它直接接到生产 API，也不要为了构建生产前端而修改这些文件。

生产前端唯一来源是 `web/`，构建并服务 `web/dist-migration/`；仓库根目录的
`legacy-web/` 已退役。本快照不参与构建、服务或默认验收。
`tests/legacy_asset_diff.py` 与 `tests/legacy_text_diff.py` 仅比较显式提供的历史
备份目录，必须同时传入 `--legacy-dir` 和 `--reference-dir`，不会默认读取生产
产物或本目录。它们不是 Vue 验收工具：

```sh
python3 tests/legacy_asset_diff.py --legacy-dir BACKUP_LEGACY --reference-dir BACKUP_REFERENCE
python3 tests/legacy_text_diff.py --legacy-dir BACKUP_LEGACY --reference-dir BACKUP_REFERENCE
```

下面保留旧前端相对冻结快照的历史基线改动登记；`legacy_text_diff.py` 在历史
比较中以此判断用户可见文本差异是否已登记，不表示当前 Vue 验收已通过。

## 有意的基线改动

- 正文中的 `http://`、`https://` 与 `www.` 网页地址可直接点击，包括加粗和斜体中的地址；
  Markdown 标记与尾随标点不进入目标地址，围栏代码保持原样，文件引用仍沿用现有识别范围。
- 报告问题弹窗沿用会话输入栏：附件卡片在输入框上方，加号、自动增高的输入框和发送键
  底边对齐；只保留机器与 CLI 选择、问题描述、附件和关闭入口，移除长说明与取消按钮。
- 控制台打开期间的重复点击使用非阻塞等待提示；控制权 HTTP 请求（含响应正文）
  在 5 秒后中止本次等待，提示服务端可能已取得控制权，清理本页忙碌状态。
  不自动重试或强制抢占，也不改变输入发送行为。超时与忙碌提示不弹原生对话框。
  按钮悬停或重绘时的提示按当前状态重新计算，含上一次失败的原因。
- 来源缺少可用 CLI 时，控制台说明写明机器名和“未配置可用的 … 启动命令”，提示检查
  CLI 安装与启动器配置后重启服务（见 docs/lifecycle-http.md）；判定条件与快照相同。
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
- 移除了“老板头像”工具图标模式：设置对话框不再有“工具图标”选择项，页面不再读写
  `sessiondock.toolIcons` 偏好，`avatars/` 目录（头像资源与其来源许可页）一并删除，
  来源图标始终显示原版图标。
- 子会话显示方式为「按需展开 / 层叠 / 平铺」三档，选择持久保存。按需模式初次仅加载
  主会话和子会话数量，保留箭头；逐层点击才加载对应分支，收起释放，刷新全收起。层叠、平铺沿用原有父子排列规则，子代理仍挂在所属
  会话下面，由三角展开或收起。打开子代理深链不替用户切换列表显示方式。
- 多选栏新增“附属到…”：把选中的会话（待定启动与分叉父行除外）一次附属到随后点选的
  同一个父会话下；点选中的会话本身不算父会话，保存失败的留在点选里可以再点一次。
- 登录环境提示：后端经 `with-zshrc` 之类的包装启动、CLI 继承后端环境后，页面每分钟询问各
  机器的 `api/shell-env`；启动文件改动导致环境变化时，底部弹出表格（机器、变化、重启、
  忽略），两台及以上时表格上方有“全部重启 (N)”；重启中的那一行原地显示“正在重启…”，
  其他行不动（见 docs/shell-env.md）。
- 弹窗统一：页面不再调用浏览器原生 `alert`/`confirm`（原 47 处），改为 `popup.js` 的
  `appAlert`/`appConfirm`——页面中央的模态对话框，与新建会话、回收站等对话框同一外观，
  消息的第一行作标题。浮动提示（版本更新、登录环境、报告问题、停止会话、控制台提示）
  统一为 `.app-float` 卡片，叠放在页面中央的 `#float-stack`，不遮挡操作；版本更新卡片
  多了“稍后”。

- 生产前端不再读取或转发 `debug_run` URL 参数；测试使用临时独立数据根。
  本目录冻结快照保留原始测试视图接线，仅作为历史基线。
- 控制台只用服务端网格渲染：移除了 xterm.js 及其 fit/unicode11/webgl 插件、浅色主题下
  的 ANSI 颜色改写、页面层的 DEC 2026 合帧和设置页“控制台渲染”选项。运行中的旧宿主
  报 `grid:false` 时，控制台说明需要重新启动会话；录制页 `records.html` 改用网格回放。
- 删除了没有主界面入口的工程页：录制页 `records.html`/`records.js`/`records.css` 和独立网格页
  `grid.html`/`grid.js`/`grid.css`。录制只在主控制台原地回放（带时间轴），网格只经
  `legacy-web/grid/facade.js` 在主控制台使用；录制与网格的 HTTP/WebSocket 接口保留。
- 不再保留非 Rust 后端的页面分支：按 tmux 名称猜测终端关联、新建会话按文件名轮询
  `api/term/new-status?name=` 和按名称停止/丢弃的旧路径一并删除；浏览器审计只发送元数据。
- 删除了用户消息下的「回到此处」和固定显示通知里的「取消固定」：它们只改网页显示，
  CLI 不回滚，容易误解。`POST api/session/rewind` 一并移除；已存的这类固定不再生效。
  Claude 在终端里自己回滚后，对话仍跟随显示到回滚点。
