# Agy（Antigravity CLI）

本页记录 agy 1.2.16 的接入证据与当前合同。2026-10-04 核对本机
`agy --help`、`agy --version` 和临时 HOME 中的真实 CLI；原生存储样本来自
仅监听 loopback 的合成 OpenAI 协议网关，未使用账号凭据或付费模型。

## 基础启动

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

新建使用 `new_pending`，不生成或猜测原生 conversation ID，不传
`--session-id`。模型和强度选择分别作为独立 argv 的 `--model`、`--effort`
传给 CLI。强度名称为 low、medium、high、xhigh、max，具体模型是否接受由
agy 判断；本地合成网关的自定义模型即不接受 effort。默认模型由 CLI 自己决定。
尚无已验证的模型目录解析，页面不填入猜测的模型名。

新建选择器、来源筛选、图标和客户端版本使用已有组件及 profile 能力门控。
未安装或未配置时禁用该节点的 Agy 选择。本次仅接入 legacy 前端，Vue 入口
保持由其开发会话维护。legacy 优先显示原生终端；可以
通过终端键盘交互、刷新后重新连接仍存活的宿主，并使用现有 pending 清理操作。

当前基础接线不宣称具备原生历史、网页 composer SEND、原生会话恢复或问题报告。
`--conversation <sid>` 已接启动器默认恢复参数，但恢复仍要求索引确认原生身份；
重新连接运行中的终端不是停止后的 CLI 恢复。未知画面维持 `unknown`，不猜测
编辑区或权限菜单按键。未绑定的 pending 清理仅处理启动回执和宿主，不删除
agy 的原生数据库。

`--version` 输出纯版本号；`update` 是 CLI 的更新子命令，沿用启动器的有界、
关闭 stdin 的手动更新路径。尚未核实独立只读的最新版本渠道，不借用其他 CLI
发布地址，不在版本查询中执行更新。

## 原生存储证据

agy 默认数据目录为 `~/.gemini/antigravity-cli/`：

- `conversation_summaries.db` 的 `conversation_summaries` 表包含 conversation ID、
  标题、workspace URI 数组、最后更新时间、运行状态和 parent conversation ID。
- `conversations/<id>.db` 是独立会话的 SQLite 库；步骤与元数据使用二进制字段。
- `brain/<id>/.system_generated/logs/transcript_full.jsonl` 是完整文本投影；
  同目录 `transcript.jsonl` 可能截断，出现 `truncated_fields` 时不能当完整正文。
- 实测完整记录包含 `step_index`、`source`、`type`、`status`、`created_at`、
  `content`；首条用户正文带 `USER_REQUEST` 与附加元数据包裹，助手类型为
  `PLANNER_RESPONSE`。内置格式说明另外声明 `thinking`、`tool_calls`、`media`。

这些调查证据不等于已经支持读取、删除、移动或克隆。原生数据库和 transcript
不是可互换的恢复载体，不能复制文本投影冒充可恢复会话。

## 验证

[agy_browser.py](../tests/agy_browser.py) 使用临时假 CLI、私有生命周期目录和
loopback 服务，覆盖页面新建、强度 argv、原生终端键入、刷新重连、pending
清理、未安装门控以及 390 px 深浅主题选择器。它不代替真实模型往返或原生
历史验收，也不运行单元测试。

官方接口依据：[会话管理](https://www.antigravity.google/docs/cli/conversations/)、
[恢复命令](https://www.antigravity.google/docs/cli/commands/resume)、
[headless 模式](https://www.antigravity.google/docs/cli/headless/)。
