# 整组操作：离线计划与暂存工具

`sessiondock-transfer` 是移动/克隆实现共用的 Rust 核心的离线入口。
当前实现范围：索引快照上的连通组计算，Codex 历史计划/暂存，以及 Claude/Grok 文件包的初步计划/暂存。
它不发布 CLI 会话、不修改源文件、不导入数据库、不切换执行归属、不执行 trash。
同节点 Codex 的 HTTP 编排与页面入口见 [当前克隆接口](session-clone.md#当前接口同节点-codex)，
使用独立能力 `session_clone_local_codex`；完整 `session_move` / `session_clone` 尚未开启。

设计见 [session-move.md](session-move.md)、[session-clone.md](session-clone.md)。

## 构建和输入

按项目的共享构建规则运行 `cargo build -p sessiondock --bins`，生成
`target/debug/sessiondock-transfer`。工具从 stdin 读取一个 JSON 请求，向 stdout 输出一个
JSON 结果；失败输出 `{ "error": { "code": "…", "message": "…" } }` 并退出 1。
所有读取根目录必须显式提供，不自动发现日常 CLI home。

### 查看索引内的整组

```json
{
  "operation": "group",
  "uid": "codex:<索引中的路径散列>",
  "roots": { "codex": "<明确配置的 Codex 历史根目录>" }
}
```

`roots` 也接受 `claude`、`grok`、`opencode`。输出 `members`、有类型的 `edges` 和
`blockers`，不按侧栏可见性过滤旧 rollout 或子代理。从父、兄弟分支或子代理发起，会得到
同一个索引连通组。`same_thread`、fork、子代理归属、续接、物理父历史双向展开。
服务中调用同一个核心时，还会读取快照行上的 `spawned_by`；纯展示的 `nest_parent` 不扩组。
离线入口不加载 SessionDock 元数据，所以不能用它代替服务最终计划的关联审计。

**此阶段的范围是已配置索引，不是完整原生文件审计**：Codex 的归档目录必须包含在显式根目录
之内，不能只给 `sessions` 然后声称检查了相邻的 `archived_sessions`。Claude 已纳入共享消息 UUID、
显式父会话、续接和 `fork-context-ref` 的父引用；Grok 纳入 summary 的 `parent_session_id`。
Grok 独立子会话同时由父目录 `subagents/<id>/meta.json` 纳入整组。
继承历史中的全部工具引用、所有原生 checkpoint 格式和跨节点关联仍在补齐。
某个无关组存在断链，不会让所有其他组失败；选中的组有已知缺失则在 blockers 中列出。

### 生成 Codex 历史计划

```json
{
  "operation": "plan_codex",
  "mode": "clone",
  "uid": "codex:<索引中的路径散列>",
  "roots": { "codex": "<包含活动及归档历史的根目录>" }
}
```

`mode` 为 `move` 或 `clone`。组内有其他来源、已知断链、无法识别的 rollout 身份或重复
物理身份时不能进入该适配器。输出包含整组、每个文件的源路径/相对路径、字节数、SHA-256、
物理历史边界及固定身份映射。克隆生成新的 thread、rollout、turn、消息/事件/工具记录 ID，并保留初始 rollout
与 thread ID 相等的关系；移动映射保持原身份。保存这份计划后再暂存，重试使用原计划。
原生工具投影的 item ID 与 call ID 可以是同一身份，因此二者使用共同的 `records` 映射，
保证调用、结果和事件中的配对不被拆开。`tool_calls` 记录原调用 ID 对应的原生工具名。

### 生成私有暂存文件

```json
{
  "operation": "stage_codex",
  "plan": { "version": 1, "...": "上一操作返回的完整计划" },
  "destination": "<已存在父目录下的全新暂存目录>"
}
```

暂存目录必须尚不存在，且不能位于源会话根目录内。工具只创建新文件；错误时可能留下部分
暂存输出，只有最终写出的 `manifest.json` 表示这一轮文件转换完成，不能据此宣称克隆可恢复。

- 移动的输出与源历史逐字节一致。
- 克隆按物理依赖顺序生成新 rollout 文件名和结构化身份字段，重新映射完整记录边界，
  修改 `history_base` 的物理 ID 和偏移；验证原 ordinal 与前缀匹配。
- 用户正文中的 UUID 不会被全文替换。已重写结构化 thread/rollout/turn 引用、root turn、
  消息元数据中的 turn、调用/结果 ID、嵌套 item、子代理事件与状态映射中的身份。
  原生 JSON 工具 `spawn_agent`、`send_input`、`wait`、`close_agent`、`resume_agent`
  的已知身份字段按工具格式重写；prompt、消息正文、完成状态正文和运行时句柄保持原文。
- 计划与暂存结果的 `reference_issues` 列出已发现但尚未适配的工具引用，包括 code-mode
  `exec` 中无法静态解析的身份引用，以及未适配工具参数/结果中的组内身份；只报告路径、ordinal 和
  原因，不把工具正文复制进诊断。没有列出问题也不表示完整审计通过。
  原生数据库投影及其他引用仍需要独立适配，不能把该列表为空当作发布许可。
- 文件摘要在读入和全部暂存结束时重新核对，源文件变化则拒绝；关系和运行态须在未来发布流程
  中重新核对，当前工具没有停写锁，也不能阻止独立 CLI 启动。
- 同一计划在两个新暂存目录生成相同身份和字节；已存在目录不覆盖。重新 plan 是另一套身份。

输出固定为 `publishable: false`，并列出 `required_checks`。这不是一个隐藏的上线开关：
原生元数据导入、完整身份引用审计、原生列表/恢复以及整组运行态重查实现并验收之前，
不能把暂存目录直接发布到日常 CLI home。

### Claude / Grok 文件包

`plan_files {uid, roots, new_ids}` 返回固定的身份映射和文件清单；
`stage_files {plan, destination}` 将清单写入一个全新的私有目录。
输出按 `claude/`、`grok/` 分区，成功时写入 `manifest.json`，始终为 `publishable: false`。
移动暂存保留原字节；新身份暂存修改已识别的结构化身份字段，保留普通正文中的旧 ID。

Claude 收集 transcript、会话附属目录、子代理 sidecar 和标准 home 下的 `file-history`；
Grok 收集整个会话目录，包括 updates、压缩记录和不解析的附件。
已补齐原生代理工具的结构化参数、结果包中的身份指针、Grok ACP 事件以及 Claude 大输出路径。
普通消息及代理答复中的旧 ID 保持原文。所有 checkpoint 格式、Claude 原生恢复与发布事务
仍未完成，不能将此输出发布为可恢复的生产会话。

## 验证

`python3 tests/session_transfer_browser.py` 调用真实 Rust 计划/暂存入口，用合成数据验证
整个连通组、移动字节不变、克隆身份/偏移、源数据不变、固定计划重试、冲突/过期计划拒绝；
随后启动隔离服务，通过 Chromium 点击暂存会话和子代理菜单，确认多层父历史仍可读取。
它不调用模型，不使用真实会话，也不代替原生 CLI 新身份恢复验收。

`python3 tests/session_files_browser.py` 构造 Claude 兄弟分支、子代理、工具结果和文件历史备份，
以及 Grok 父子分支、updates 和压缩文件；验证整组范围与源数据不变，再用 Chromium
打开移动/复制暂存后的各分支和 Claude 子代理。全部数据与服务使用私有临时目录。

`python3 tests/session_files_grok_real.py` 是显式运行的原生隔离实验：使用 `grok-4.6` low
创建父会话、fork 和子代理，将暂存输出实验性导入同一个临时 home，验证克隆后的父会话及
子代理按新 ID 续聊，再继续原组并核对两组互不追加。断言原生 chat 记录中的实际模型
`grok-4.6-build` 和 low 强度、原生关系以及日常配置不变。结果写入
`target/session-files-grok-report.json`；不代表生产发布或跨机验收通过。
