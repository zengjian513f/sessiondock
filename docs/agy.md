# Agy（Antigravity CLI）

本页记录 agy 1.2.16/1.2.17 的接入合同与证据边界，核对日期为 2026-10-05。
接入范围是 Rust 后端与 legacy 前端。

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
ID 以强度级别结尾的条目（如 `gemini-3.8-flash-high`、`gemini-3.1-pro-low-thinking`）
本身就是强度变体，原始目录行的 `efforts` 为空。网页将同一基础模型的强度变体
合并成左侧一项，右侧 effort 只允许目录中存在的组合，缺失档位置灰禁用。
提交时映射回对应的原生模型 ID，启动不再另带 `--effort`；原生基础模型行存在时
也提供“CLI 默认”。旧的已保存变体 ID 自动恢复为基础模型及对应强度。
目录按 profile 缓存在服务内存：CLI 探测（启动及每 5 分钟）在后台预热缺失或超过
10 分钟的目录；首次请求复用正在预热的查询，过期目录照常返回并只触发一次后台刷新；
空结果不覆盖已有目录，失败后的重试也遵守刷新间隔。
选择模型用独立 argv `--model <id>`；可选强度用 `--effort <level>`，提供
low、medium、high、xhigh、max，由 CLI 判断没有强度变体元数据的模型是否接受。
这些普通模型的 Agy 选择器支持空值
“CLI 默认”，省略 `--effort`；合成网关的自定义模型测试使用此空值。
未选模型时也省略模型 override。CLI 的后台 title generator 可能另选目录中的
第二个合成模型，验收应分别检查 interactive planner 和 title 请求，不能据标题请求
推断主会话模型选错。

新建选择器、来源筛选、图标和客户端矩阵沿用已有组件及 profile 能力门控。
未安装或未配置时禁用该节点的 Agy 选择。未绑定 pending 清理只处理启动回执和
宿主，不删除原生数据库。当前浏览器套件针对 legacy 前端。

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
  `transcript_missing: true`。索引摘要与历史详情按缓存状态生成相同提示：已有记录时说明
  “保留上次读取的历史”；没有记录时说明仅找到会话索引、没有正文与缓存，暂时无法
  显示消息，并建议在 Agy 中确认该会话是否仍可打开。从未导出过时才建立空消息文件。
  文件重现后重新核对并清除提示，不能把缺失当回退或空会话，也不能在没有缓存时
  声称保留了历史或承诺原文件必然恢复。打开的详情页随增量元数据刷新提示，文件恢复
  后无需重新加载页面即可清除旧提示。
- 成功读取目录后 SID 行不再存在：只清理带本实现标记的私有镜像，原生 transcript
  和会话库保持原样。重启也清理这些已失去目录行的旧镜像。

## 历史投影与证据

真实 agy 1.2.16 在临时 HOME、仅监听 loopback 的合成 OpenAI 协议网关中产生了
原生样本，未使用账号凭据或付费模型。已核对 native user、planner final、thinking、
`ERROR_MESSAGE` 和原生会话 DB 的 fd identity。

- `USER_INPUT`：仅去除 CLI 外层 `<USER_REQUEST>`，保留用户自己的正文与内层
  同名文本，作为原生回显对账依据。尾部 `<ADDITIONAL_METADATA>` 识别为
  `native_metadata`，不混入用户正文；`<USER_SETTINGS_CHANGE>` 独立显示为
  “设置变更”的系统气泡。其他完整尾部 tag 保留类型和正文，显示“附加信息”；
  不完整结构保留原文，不按正文里的代码/HTML tag 推断系统消息。
- `SYSTEM_MESSAGE`：独立投影为 `system`，使用既有系统气泡并标注“系统消息”，
  不进入工具组、不计为回复。仅移除原生说明前缀和完整外层 `<SYSTEM_MESSAGE>`；
  `[Message]` 头的 timestamp/sender/priority 单独保留在消息字段与标注提示中，
  content 显示为正文。后台任务完成通知沿用系统角色，不冒充工具结果或用户输入；
  纯文本和不完整包裹保留可见正文。
- `PLANNER_RESPONSE`：`thinking` 单独投影；无 `tool_calls` 且 status 为 `DONE`
  时正文为 `phase: final`，其他情况为 `progress`。`ERROR_MESSAGE` 投影为可见错误消息。
- 所有投影消息携带 `native_type`。`GENERIC` 沿用独立工具结果；未知原生步骤仍
  保留类型名称与原始内容，不猜工具配对或丢弃内容。上述识别覆盖本次报告会话实际
  出现的记录与包裹，不宣称识别尚未见到的任意 tag。
- `tool_calls` 和 `media` 已取得真实 CLI 样本：`view_file` 调用包含 `name` 与 `args`，
  后续 GENERIC 步骤提供文本或图片结果。调用保留 native JSON，结果独立显示；
  完整 transcript 的这些实测记录没有跨步骤 call ID，不按位置猜配对，也不宣称
  已验证其他工具、并行工具或可靠的调用/结果配对。
- 图片按 `media[].mime_type` 和 `uri` 接入既有媒体授权/文件校验，其他媒体类型显示
  跳过提示。真实 CLI 读取临时 PNG 后写出的绝对路径 `media.uri`，已通过 Chromium
  实际打开会话、展开工具结果并加载图片；不只依赖合成 schema 夹具。

工具验收仍使用本机合成网关，不使用账号或付费模型。未知自定义模型名的原有实验
没有导出工具；本次用 CLI 已知的 `gemini-3.1-pro-low-thinking` 元数据，显式启动
该模型并核对请求中的映射名 `gemini-3.1-pro-preview`，网关只返回两次读取临时文件
的 `view_file`。模型名用于选取 CLI 能力元数据，所有请求仍由 loopback 网关响应。
测试设置仅写临时 HOME，不改变日常模型或网关配置。

`instance.busy` 从已识别编辑区下方的原生状态判断：running 后台任务栏或左侧
`esc to cancel` 为忙碌；没有任务栏时，`? for shortcuts` 为闲置，非空草稿下只剩
右对齐模型标签时也为闲置。正文、草稿中的
同名文字不参与判断；菜单或未知布局返回 `null`。真实隔离请求的进行中、完成及
非空草稿画面已核对，浏览器检查同一状态与会话头的工作指示。
摘要仍不输出 `turn`：实测 Esc 中断流式输出后，目录也会回到 IDLE，已产生的
PLANNER_RESPONSE 仍为 DONE，与正常完成无法仅凭这些字段区分；没有独立原生
中断记录。`phase: final` 不代表整个回合正常完成，不据此伪造中断/完成三态。

## Composer 与原生菜单

已实现 agy 1.2.16 的正向编辑区识别：匹配上下相同的完整横线、`> ` 首行、续行
缩进及区内光标，底部为单行页脚，或 1.2.17 的后台任务栏（带时间的 `●` running 行、
同宽分隔线与 `/tasks` 页脚）；不按模型名子串授权发送。
BUG-20261005-150140-776952：后台命令已在原生记录中进入 RUNNING，终端仍有空编辑区，
旧识别器因下方超过一行而使 CHECK 返回 409。任务栏现参与同一 CHECK/SEND 识别，
并令 `instance.busy` 为 true；任意正文、缺失分隔线及框外光标仍不恢复编辑区。
[菜单浏览器套件](../tests/cli_menus_browser.py) 用私有假 CLI 重放该布局，覆盖网页发送、
终端已有草稿、任务结束及菜单拒发。
空编辑区可进入普通 CHECK/SEND，非空编辑区返回 `cli_input_pending` 保留输入。
可见正文写入 `cli.editor.text`；原生 user 镜像用于 SEND 回显确认，多行发送复用
粘贴后画面再检。真实 CLI 浏览器覆盖普通正文、多行正文以及完整问题报告正文。
网页发送精确的裸命令 `/model`、`/permissions`、`/resume`、`/help`、`/settings` 时，CLI 打开菜单而不写
原命令的 `USER_INPUT`；这些发送只确认终端投递，不建立等待原生回显的队列项。
带参数、空白变体和其他命令尚未核实分派语义，仍沿用普通输入对账。

当前菜单投影支持 model 单选/取消、workspace trust 的信任/退出选择、permissions
scope 的 Project / Shared with Antigravity / Global 选择。沿用 `screen_menu`
题卡和当前焦点推算的 Up/Down/Enter；model 与 scope 提供 Escape，trust 不猜取消键。

1.2.17 的命令审批保留完整命令与四种原生范围：单次、当前会话、持久允许、拒绝；
Escape 中断。文件创建审批保留路径、可见 diff 与允许/拒绝选项。Tab Amend 打开的是
批准并补充下一步说明，仍在终端输入；文件审批的 `f` 打开完整 diff，Escape 返回。
`ask_question` 支持当前页单选、多选 Space 勾选、前后题切换和明确提交；`Write-in...`
进入自填页后，仅空白原生输入框提供网页填写。终端已有答案时保留 native 编辑路径。
语义 ID 包含命令、diff、题目和授权范围，焦点或多选勾选变化只更新 revision；
点击前重新 CHECK，命令或题目变化后不得沿用旧题卡授权。

已识别菜单先于编辑区判断并阻止普通 SEND。未知菜单（包括权限深层编辑、settings、
help、resume 的复杂列表操作）、Amend 文本与完整 diff 视图保留原生终端路径。
编辑区未识别时拒发并保留网页草稿，不把未知画面当普通输入框。

BUG-20261005-092243-45bf46：原生 `run_command` 已进入 Command 审批，绑定正常，
浏览器 CHECK 随后返回 409；旧解析器只接受 Keyboard/model、trust、scope 的页脚，
没有识别 `↑/↓ Navigate · tab Amend · ctrl+g edit/expand command`，因此未提供题卡。
修复位于服务端画面投影，同一结果用于 CHECK、题卡与 SEND 门控，不修改原生记录。

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
| [agy_real_browser.py](../tests/agy_real_browser.py) | 直接显式运行 `python3 tests/agy_real_browser.py --agy <absolute CLI path> --binary target/debug/sessiondock --ptyhost target/debug/ptyhost`；临时 HOME/XDG、私有目录和 loopback 合成 gateway，真实 CLI + Chromium | operator only，`run_validation: skip`；验证发送、多行、绑定、思考、网页发送菜单命令不等待原生回显、菜单阻发/回答/取消、停止恢复、问题报告与窄屏；不调用付费模型 |
| [agy_tools_real_browser.py](../tests/agy_tools_real_browser.py) | 直接显式运行并提供 `--agy <absolute CLI path> --binary target/debug/sessiondock`；真实 CLI 读取临时文本和 PNG，再由 Chromium 展开原生工具历史和加载图片 | operator only，`run_validation: skip`；loopback 合成模型，核对实际请求模型；不证明跨步骤配对或其他工具语义 |
| [agy_clients_browser.py](../tests/agy_clients_browser.py) | 本机与 Hub 的模型、强度 argv、版本、官方 manifest、更新成功/失败、未安装与离线门控 | 仅运行私有假 CLI/curl，不修改已安装客户端 |
| [cli_menus_browser.py](../tests/cli_menus_browser.py) | `--sources agy`；28 个捕获/边界夹具，含本次脱敏报告画面 | Chromium 点击选项、操作和文本提交，验证过期画面、命令/信任路径变化及同名宿主实例替换不写入；回放保留捕获列宽 |
| [agy_interactions_real_browser.py](../tests/agy_interactions_real_browser.py) | 显式 `--agy PATH --binary PATH --ptyhost PATH`；真实 1.2.17、私有 HOME 和 loopback 模型；审批、问卷、自填及五个菜单命令 | 实际 wire 模型为 `gemini-3.1-pro-preview`；验证单次/拒绝/取消/补充说明、会话/持久授权及后续命令、创建文件、多选和前后题；无账号或付费请求 |

能力限制：可靠工具配对、回合三态、原生子代理及非交互删除能力尚未证实。
历史工具/媒体验收覆盖 `view_file` 的文本和 PNG；交互验收另覆盖临时命令、创建文件
与 `ask_question`，不能推广为所有原生工具行为。未完成的交付项记录于 [TODO](../TODO.md)。
