# 部署 `sessiondock-hub`（多机 Hub，仅样例）

实际节点身份、两套 WireGuard 的边界和路由核对方法见
[网络拓扑](network-topology.md)。尤其不要把另一套网络的中转站
`192.168.2.10` 当作 Cetus。

本文只给 systemd 单元与 nginx 位置的**形状**，用占位符代替一切地址、路径与凭据。它不是生产授权：
切生产流量、动 `deploy/*`、接旧 Hub 都是单独的、需用户授权的步骤（[replacement-checklist.md](replacement-checklist.md)）。
Hub 自身**无鉴权**，只 loopback 绑定，放在**已鉴权的反代**后面（和节点侧一样）。

## 边界

- 生产发布必须同批部署 Hub 和全部节点；任何一端未部署，发布都不算完成。
- Hub 是独立二进制 `sessiondock-hub`，**不共享节点的任何私有目录**：不读会话根、host、delivery、
  lifecycle、state、audit（节点的）。它只拥有自己的注册表、缓存目录和（可选）审计目录。
- 注册是**服务器端操作**：`sessiondock-hub register/remove/list`，网页上没有注册路由。节点凭据从
  文件读（`--token-file`），绝不进程列表可见。
- Hub↔节点走私网（WireGuard）；节点开**第二监听**（`SESSIONDOCK_NODE_BIND` 等，见
  [security-model.md](security-model.md#node-listener-hub-traffic-batch-38-h1)），Hub 以私网字面 IP 注册节点。
- 缺任何必填配置即启动失败（fail closed）。启动前先 `sessiondock-hub --check-config`（与启动同样的
  校验，只打印生效值，不绑不注册不开审计）。

## 环境变量

| 变量 | 说明 |
| --- | --- |
| `SESSIONDOCK_HUB_BIND` | Hub 监听，必须 loopback；默认 `127.0.0.1:8742` |
| `SESSIONDOCK_HUB_NODES` | `hub-nodes.json` 注册表；默认 `~/.local/share/sessiondock/hub-nodes.json`，接受普通文件路径 |
| `SESSIONDOCK_HUB_CACHE_DIR` | 离线会话快照目录；缺省是注册表旁的 `hub-cache` |
| `SESSIONDOCK_HUB_NETWORKS` | 可注册的节点 CIDR；默认 `127.0.0.0/8,::1/128,10.0.0.0/24`，与 Python 一致 |
| `SESSIONDOCK_WEB_DIR` | 前端快照（hub 模式），默认 `legacy-web` |
| `SESSIONDOCK_AUDIT_DIR` | 记 `hub.node.*.changed`（可选，按需创建目录） |

## 注册（服务器端）

```sh
# 把节点凭据写进只读文件（Hub 与节点共享同一个 token 文件内容），再注册；地址是节点第二监听的私网 IP。
umask 077; printf '%s' "<NODE_TOKEN>" > <TOKEN_FILE>
sessiondock-hub register --name <MACHINE_NAME> --url http://<NODE_WG_IP>:<NODE_PORT> --token-file <TOKEN_FILE> [--color teal]
sessiondock-hub list           # 核对注册表（不含凭据）
sessiondock-hub remove <NODE_ID>
```

注册时 Hub 会打节点的 `/api/meta`，必须经过节点第二监听（带 token 与 `X-SessionDock-Protocol: 1`）、
且节点自报 `mode:"local", protocol:1, node_id` 32 hex，否则拒绝。

## systemd 单元（样例）

```ini
[Unit]
Description=SessionDock multi-machine hub (loopback, behind the authenticated reverse proxy)
Wants=network-online.target
After=network-online.target

[Service]
Type=simple
# 装成 user service；工作目录、二进制路径、环境文件按 hub 宿主机实际填。
WorkingDirectory=<HUB_PREFIX>
ExecStart=<HUB_PREFIX>/bin/sessiondock-hub
EnvironmentFile=<HUB_PREFIX>/etc/hub.env      # 上表中的 SESSIONDOCK_HUB_* 等
Restart=on-failure
RestartSec=2
UMask=0077

[Install]
WantedBy=default.target
```

`hub.env` 只列变量名与占位路径，不提交真实地址/凭据（[AGENTS.md](../AGENTS.md#scope-and-boundaries)）。

## nginx 位置（样例，放在已鉴权的 HTTPS server 内）

```nginx
# 在已登录鉴权的 server 块里加一个位置；<HUB_PATH> 是挂载前缀（页面 storage 命名空间按它区分）。
location = <HUB_PATH> { return 301 <HUB_PATH>/; }

location <HUB_PATH>/ {
    include snippets/auth.conf;              # 登录鉴权（Hub 自身无鉴权）
    proxy_pass http://127.0.0.1:<HUB_PORT>/;
    proxy_http_version 1.1;
    proxy_set_header Host $http_host;
    proxy_set_header X-Real-IP $remote_addr;         # Hub 只用于 ownership 显示，不参与鉴权
    proxy_set_header X-Forwarded-Proto $scheme;
    proxy_set_header Upgrade $http_upgrade;           # /api/term/attach WebSocket
    proxy_set_header Connection $connection_upgrade;  # map $http_upgrade → "upgrade"/""
    proxy_buffering off;
    proxy_request_buffering off;                      # SSE / NDJSON 搜索 / 大附件
    client_max_body_size 512m;                        # ATTACHMENT_MAX_BYTES
    proxy_read_timeout 3600s;
    proxy_send_timeout 3600s;
    add_header Cache-Control "no-store" always;
}
```

- WebSocket 需要透传 `Upgrade` 和 `Connection`。关闭代理缓冲，保证 SSE
  和 NDJSON 逐行到达浏览器。
- `client_max_body_size` 覆盖附件上限；读写超时给长连接（终端、SSE）留足。
- 反代把 Host 原样传给 Hub（Hub 的同源检查按 Host 核对 Origin）；Hub 只 loopback，不直接对外。

回退与切流步骤见 [replacement-checklist.md](replacement-checklist.md)。

同节点反代时，把外部 Host 加入 `SESSIONDOCK_PUBLIC_HOSTS`，否则 Hub
返回 403 `local_only`。用 `sessiondock-hub --check-config` 核对。
