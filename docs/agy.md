# Agy（Antigravity CLI）

本页记录 agy 1.2.16 的接入合同与证据边界，核对日期为 2026-10-04。
接入范围是 Rust 后端与 legacy 前端；Vue 由独立开发任务维护。

官方接口依据：[会话管理](https://www.antigravity.google/docs/cli/conversations/)、
[恢复命令](https://www.antigravity.google/docs/cli/commands/resume)、
[headless 模式](https://www.antigravity.google/docs/cli/headless/)。

## 启动、模型与恢复

来源为 `agy`，显示名为 Agy。每台节点配置唯一一个 CLI profile：

```json
{
  "id": "agy-cli-v1",
  "source": "agy",
  "executable": "<absolute path to agy>",
  "args": [],
  "resume_args": ["--conversation", "{sid}"]
}
```

新建使用 `new_pending`，不生成或猜测原生 conversation ID，不传 `--session-id`。
原生行出现后，索引的完整 SID 和 CLI 打开的 `conversations/<sid>.db` 文件描述符
提供身份依据；绑定沿用宿主进程证据路径，不能仅凭 cwd 配对。
停止后恢复的默认 argv 为 `--conversation <sid>`，SID 来自已验证的索引身份。
真实 CLI 浏览器验证包含停止、启动新宿主恢复同一 SID、再次网页发送与原生回复。

模型目录执行 profile 环境中的 `agy models`（15 秒上限），解析 stdout 的
`id\tdisplay name` TSV；失败不填入猜测模型。目录没有默认模型或逐模型强度元数据。
选择模型用独立 argv `--model <id>`；可选强度用 `--effort <level>`，提供
low、medium、high、xhigh、max，由 CLI 判断模型是否接受。Agy 选择器支持空值
“CLI 默认”，省略 `--effort`；合成网关的自定义模型测试使用此空值。
未选模型时也省略模型 override。CLI 的后台 title generator 可能另选目录中的
第二个合成模型，验收应分别检查 interactive planner 和 title 请求，不能据标题请求
推断主会话模型选错。

新建选择器、来源筛选、图标和客户端矩阵沿用已有组件及 profile 能力门控。
未安装或未配置时禁用该节点的 Agy 选择。未绑定 pending 清理只处理启动回执和
宿主，不删除原生数据库。当前浏览器套件针对 legacy 前端；不据此宣称 Vue 已验收。

## 原生存储与只读镜像

agy 默认数据目录为 `~/.gemini/antigravity-cli/`，SessionDock 不自动发现它。
`SESSIONDOCK_AGY_HOME` 必须显式指向原生数据目录，与私有可写的
`SESSIONDOCK_AGY_ROOT` 成对配置，否则启动配置报错。只读来源为：

- `conversation_summaries.db` 的 `conversation_summaries` 表：SID、标题、workspace
  URI、更新时间、status、parent conversation ID、step count、agent name。
- `brain/<sid>/.system_generated/logs/transcript_full.jsonl`：完整文本记录。
  同目录 `transcript.jsonl` 可能带 `truncated_fields`，不作为正文来源。
- `conversations/<sid>.db`：agy 的原生二进制会话库；用于 CLI 恢复与 fd 身份证据，
  SessionDock 不解码或修改其中步骤。

镜像位于 `<AGY_ROOT>/cli/<sid>/summary.json` 和 `messages.jsonl`，标记
`sessiondock-agy-mirror`。新建私有目录/文件采用 0700/0600。列表、历史分页、
增量视图、搜索和媒体复用文件读模型，不复制镜像冒充原生恢复载体。
列表标题优先使用目录标题，否则取首条 user 正文，再回退 `Agy 会话`；cwd 解码
workspace 的 file URI。当前目录查询没有模型字段，列表不能据启动 argv 猜造模型；
读取到的 parent/status 字段也不代表已实现原生子代理挂靠或回合三态。

持久只读 SQLite 连接以 `PRAGMA data_version` 检测目录提交；在读事务前采样，
整轮同步成功后确认，后续提交留给下一轮。每秒查看已知 transcript 的
dev/ino、size、mtime stamp，目录未提交也能发现正文追加或改写；不遍历其他 HOME。
正文只导出以 LF 结束的完整行；不凭未证实的 status 名称截断后续记录。
增长且旧字节完全为前缀时追加，旧记录改写或回退时原子重写。
相同字节不重写镜像；目录 DB 原子替换、连接恢复和服务重启会重新核对。

缺失与删除有不同含义：

- 完整 `conversation_summaries.db` 暂时缺失：关闭连接、重置版本证据，保留已有
  镜像，等待 DB 重现；不是所有会话被删除。
- 目录行仍存在但完整 transcript 缺失：保留上次消息镜像，摘要标记
  `transcript_missing: true`，历史详情提示“保留上次读取的历史，等待原生文件恢复”；
  从未导出过时才建立空消息文件。文件重现后重新核对并清除提示，不能把缺失当回退
  或空会话。
- 成功读取目录后 SID 行不再存在：只清理带本实现标记的私有镜像，原生 transcript
  和会话库保持原样。重启也清理这些已失去目录行的旧镜像。

## 历史投影与证据

真实 agy 1.2.16 在临时 HOME、仅监听 loopback 的合成 OpenAI 协议网关中产生了
原生样本，未使用账号凭据或付费模型。已核对 native user、planner final、thinking、
`ERROR_MESSAGE` 和原生会话 DB 的 fd identity。

- `USER_INPUT`：仅去除 CLI 外层 `<USER_REQUEST>` 与附加 metadata 包裹，保留用户
  自己的正文与内层同名文本，作为原生回显对账依据。
- `PLANNER_RESPONSE`：`thinking` 单独投影；无 `tool_calls` 且 status 为 `DONE`
  时正文为 `phase: final`，其他情况为 `progress`。`ERROR_MESSAGE` 投影为可见错误消息。
- `tool_calls` 和 `media` 字段来自 CLI 内置格式文档，尚无成功真实工具/媒体样本。
  当前调用保留 native JSON，其他有正文的步骤独立显示为工具结果；没有已核实的
  跨步骤 call ID，不按位置猜配对，也不宣称真实工具配对已通过。
- 图片按 `media[].mime_type` 和 `uri` 接入既有媒体授权/文件校验，其他媒体类型显示
  跳过提示。图片的合成 schema history browser 已通过；这不是 CLI 实际媒体产出证据。

`instance.busy` 尚无 Agy 判定，返回 `null`；摘要不输出 `turn`。
不从原生 status、没有回复或进程存活猜测忙碌、空闲、中断和回合完成。

## Composer 与原生菜单

已实现 agy 1.2.16 的正向编辑区识别：匹配上下相同的完整横线、`> ` 首行、续行
缩进及区内光标，底部最多一条非空页脚；不按模型名子串授权发送。
空编辑区可进入普通 CHECK/SEND，非空编辑区返回 `cli_input_pending` 保留输入。
可见正文写入 `cli.editor.text`；原生 user 镜像用于 SEND 回显确认，多行发送复用
粘贴后画面再检。真实 CLI 浏览器覆盖普通正文、多行正文以及完整问题报告正文。

当前菜单投影支持 model 单选/取消、workspace trust 的信任/退出选择、permissions
scope 的 Project / Shared with Antigravity / Global 选择。它们沿用 `screen_menu`
题卡和当前焦点推算的 Up/Down/Enter；model 与 scope 提供 Escape，trust 不猜取消键。
已识别菜单先于编辑区判断并阻止普通 SEND。未知菜单（含 permissions 深层编辑、
未投影的 `/resume` 菜单）回退原生终端，未识别编辑区时拒发并保留网页草稿。
scope 选择不等于已支持工具审批、多选、文本表单或整个权限编辑器。

## 操作、更新与问题报告

原生删除、移动、克隆均不支持。删除返回 `409 agy_delete_unsupported`，提示在 CLI
`/resume` 菜单操作；真实 1.2.16 的该菜单提供 `f4 Delete`，但尚无 verified
noninteractive delete API，不能删 mirror 冒充 native 删除。含 Agy 的整组操作必须
按既有能力合同显示阻碍项，不能忽略该成员或仅迁移文本镜像。

`--version` 输出纯版本号。客户端 latest 已实现读取官方 updater 的平台 manifest
`version`（OS/arch，Linux musl 使用对应后缀），沿用后台缓存；手动更新执行
`agy update`，关闭 stdin 并遵守现有有界更新合同。本机与 Hub 浏览器用替身验证更新成功、失败和关闭 stdin；没有为验收升级真实 CLI。
问题报告的服务端允许来源及 legacy 选择器已加入 Agy，复用普通模型选择、启动与
composer 路径；真实 CLI 浏览器核对了处理会话、完整报告提示及原生回复。

## 验证方法与未决项

下列验证使用私有运行目录和 loopback 服务：

| 套件 | 数据与范围 | 证据边界 |
| --- | --- | --- |
| [agy_browser.py](../tests/agy_browser.py) | `python3 tests/agy_browser.py --binary target/debug/sessiondock`；fake CLI pending，新建、effort argv、终端键入、刷新重连、清理、未安装与 390 px 深浅主题 | 不证明 native history 或真实 CLI 恢复 |
| [agy_history_browser.py](../tests/agy_history_browser.py) | `python3 tests/agy_history_browser.py --binary target/debug/sessiondock`；native schema seeds，Chromium 列表/搜索/分页/增量/旧记录改写/回退/DB 替换/重启/transcript 缺失与恢复/目录行删除/图片/原始工具字段/错误、删除提示及混合组移动克隆拒绝，检查原生字节及 mtime 不被修改 | 合成 schema，图片路径已有通过记录；不证明真实工具或媒体产出，不是原生删除 API 测试 |
| [agy_real_browser.py](../tests/agy_real_browser.py) | 直接显式运行 `python3 tests/agy_real_browser.py --agy <absolute CLI path> --binary target/debug/sessiondock --ptyhost target/debug/ptyhost`；临时 HOME/XDG、私有目录和 loopback 合成 gateway，真实 CLI + Chromium | operator only，`run_validation: skip`；验证发送、多行、绑定、思考、菜单阻发/回答/取消、停止恢复、问题报告与窄屏；不调用付费模型 |
| [agy_clients_browser.py](../tests/agy_clients_browser.py) | 本机与 Hub 的模型、强度 argv、版本、官方 manifest、更新成功/失败、未安装与离线门控 | 仅运行私有假 CLI/curl，不修改已安装客户端 |
| [cli_menus_browser.py](../tests/cli_menus_browser.py) | `--sources agy`；12 个捕获/边界夹具、10 个原生按键动作 | Chromium 验证过期画面、信任路径变化及同名宿主实例替换不写入；夹具不声称覆盖全部闭源菜单 |

能力限制：真实工具/媒体成功样本、可靠工具配对、busy/turn、原生子代理及非交互
删除能力尚未证实。原生工具 JSON 和媒体 schema 夹具证明字段投影，不证明这些
原生行为。未完成的交付项记录于 [TODO](../TODO.md)。
