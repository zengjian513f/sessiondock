# 会话迁移传输层

传输层只交付私有 tar 包。计划、确认、锁、最小字节身份改写、目标验证、发布、执行归属交接
及源端清理仍遵守 [移动](session-move.md) 和 [复制](session-clone.md) 合同。
支持 `hub` 默认中转与 `scp` 网络组直传；未实现 rclone、多路并行及断点续传。

## 配置

Hub 环境变量 `SESSIONDOCK_HUB_TRANSFER_CONFIG` 指向管理员维护的 JSON 文件。
未设置时保持原有 Hub 中转。设置后启动与 `--check-config` 都读取并校验文件；修改配置需重启 Hub。
使用 JSON 与现有注册表配置保持一致，不需要新增 YAML 依赖。真实地址和此配置文件不入库。

```json
{
  "default": "hub",
  "networks": [
    {
      "name": "compute-lan",
      "transport": "scp",
      "priority": 100,
      "nodes": {
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa": {"address": "node-a.internal", "user": "operator", "port": 22},
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb": {"address": "node-b.internal", "user": "operator", "port": 22}
      }
    }
  ]
}
```

节点键必须替换成注册表中的持久节点 ID，地址、SSH 用户和端口明确填写。
`address` 支持主机名、IPv4 和未加方括号的 IPv6。网络组是管理员声明的可直连范围，
不按 CIDR、主机名、地址后缀或 Hub 可达性自动推断。组名用于页面显示。
源与目标共同所属组中，`priority` 最大者优先（缺省 0）；同优先级按配置先后选择。
没有共同组、未配置或旧节点缺少 `scp_transfer` 能力时使用 Hub。
同节点复制保持原有本地流程。

目标节点以服务用户运行 `scp`，用该用户已有的 SSH 密钥和 `known_hosts` 拉取源节点。
需要可用的 SCP/SFTP 客户端、源端 SFTP 服务和目标到源的免密连接；反向迁移相应要求反向连接。
强制非交互认证、严格主机密钥检查，不自动接受未知主机密钥，不转发 SSH agent，
不向节点分发 Hub API token，不修改用户日常 SSH 配置。地址使用所选网络组配置，
实际路由由系统决定；WireGuard 地址本身不保证绕过其中继。

## 执行与回退

1. Hub 完成原有源预留、清单准备及目标检查，再选路并持久记录选择。
2. SCP 路径调用源端私有 `scp/prepare`。源端执行与 HTTP 导出相同的打包、停写和快照复核，
   返回本次操作私有目录下的归档路径与长度。
3. Hub 调用目标私有 `scp/receive`；目标 SCP 拉取到自己的私有临时目录，随后进入共用的
   tar 接收、清单身份匹配、内容校验及暂存逻辑。SCP 不写 CLI 根目录，也不执行发布或源清理。
4. Hub 释放源端归档，再按原事务完成发布和后续步骤。界面复用现有进度明细，显示
   `SCP 直传 · 网络组` 或 `Hub 中转`、回退原因及真实处理进度；journal 保存 `transport`。

SCP 无法启动、连接失败或传输连续 60 秒没有文件长度进展时，先终止并回收 SCP/SSH 子进程，
删除本次未完成归档，再由 Hub 核对目标持久接收状态。目标没有接收记录时回退到 Hub 中转；
已有有效接收记录时继续原事务。校验失败、操作冲突及取消不回退。
没有总文件大小或总传输耗时上限。

HTTP 响应丢失时，Hub 经私有 `scp/settle` 等待接收工作结束，再查询原操作。
已接收则继续，状态仍不确定则交给既有撤回/恢复流程，不盲目重传。
取消先设置中断标志、等待 SCP 结束并清理部分归档，再执行原补偿流程；Hub 重启仍按原
journal 恢复规则处理。节点重启清理未落账的私有 `incoming-*` 目录。
私有 `scp/prepare`、`receive`、`settle`、`release` 都位于 `/api/session/transfer/` 下，
只挂载在鉴权节点监听器，浏览器通用代理拒绝访问。

## 验证

`tests/session_transport_browser.py` 用 Chromium 真正选择目标、确认复制、打开目标历史和取消。
隔离的模拟 SCP 覆盖网络优先级、无共同网络、旧节点、部分失败后回退、损坏包不回退、
响应丢失、完成后重启重试、执行中 Hub 重启及取消后的进程/暂存清理。

真实两机 SCP 复用 `tests/session_bundle_browser.py` 的临时目录和浏览器验收：

```sh
python3 tests/session_bundle_browser.py --binary <BINARY> --peer <SSH_ALIAS> \
  --scp-source <SOURCE_ADDRESS> --scp-target <TARGET_ADDRESS> \
  --scp-user <SSH_USER> --scp-port <SSH_PORT>
```

可加 `--move`、`--preserve`，执行三种 CLI 的移动、保留身份与移回验收。
只使用临时合成会话，不拿生产会话测试。
