# 整组操作：离线计划与暂存工具

`sessiondock-transfer` 是移动/克隆实现共用的 Rust 核心的离线入口。
当前实现范围：索引快照上的连通组计算，以及 **Codex 原始历史文件**的计划和暂存。
它不发布 CLI 会话、不修改源文件、不导入数据库、不切换执行归属、不执行 trash。
HTTP 编排与页面入口尚未接入，`session_move` / `session_clone` 能力尚未开启。

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
之内，不能只给 `sessions` 然后声称检查了相邻的 `archived_sessions`。Claude 跨文件 fork
关系、继承历史中的工具/子代理引用、其他来源完整文件集及跨节点关联仍待来源适配器补齐。
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
  `exec` 中的 JavaScript，以及未适配工具参数/结果中的组内身份；只报告路径、ordinal 和
  原因，不把工具正文复制进诊断。没有列出问题也不表示完整审计通过。
  原生数据库投影及其他引用仍需要独立适配，不能把该列表为空当作发布许可。
- 文件摘要在读入和全部暂存结束时重新核对，源文件变化则拒绝；关系和运行态须在未来发布流程
  中重新核对，当前工具没有停写锁，也不能阻止独立 CLI 启动。
- 同一计划在两个新暂存目录生成相同身份和字节；已存在目录不覆盖。重新 plan 是另一套身份。

输出固定为 `publishable: false`，并列出 `required_checks`。这不是一个隐藏的上线开关：
原生元数据导入、完整身份引用审计、原生列表/恢复以及整组运行态重查实现并验收之前，
不能把暂存目录直接发布到日常 CLI home。

## 验证

`python3 tests/session_transfer_browser.py` 调用真实 Rust 计划/暂存入口，用合成数据验证
整个连通组、移动字节不变、克隆身份/偏移、源数据不变、固定计划重试、冲突/过期计划拒绝；
随后启动隔离服务，通过 Chromium 点击暂存会话和子代理菜单，确认多层父历史仍可读取。
它不调用模型，不使用真实会话，也不代替原生 CLI 新身份恢复验收。
