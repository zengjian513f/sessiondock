# 运维：节点身份与网络边界

部署地址、SSH 用户与端口以未跟踪的 `deploy/targets.local.json` 和本机 SSH 配置为准；
Hub 控制通道以其节点注册表为准。被跟踪文档只描述配置形状，不保存生产地址或在线状态。

## 身份规则

- Hub→节点控制通信使用配置的私网；显式配置的
  [SCP 数据传输](session-transport.md) 可使用管理员指定的直连地址。
  数据传输地址不改变节点注册和控制通道。
- 中继主机与 SessionDock 节点是独立身份。不同网络中的相同主机号后缀不能用于判断
  它们是同一台机器；不得据此替换 Cetus 或其他节点的地址。
- SSH 别名、实际 hostname、节点 `/api/meta` 的 `node_id`、Hub 注册表必须对应。
  节点是否在线用 [舰队状态工具](deployment.md) 读取，不从文档中的固定列表推断。

## 路由核对清单

1. 从本地清单和 Hub 注册表取得目标地址、SSH 端口与预期身份。
2. 在支持该命令的平台用 `ip route get <NODE_IP>` 检查实际路由，核对接口和私网配置。
3. Hub 访问节点的第二监听，形状见 [deploy-hub.md](deploy-hub.md) 与
   [security-model.md](security-model.md#node-listener-hub-traffic)。
4. 身份或路由不符时先纠正配置；不能用另一套网络中后缀相同的地址代替。
