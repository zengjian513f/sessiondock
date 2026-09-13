# 部署 `sessiondock-hub`（多机 Hub，仅样例）

本文只给 systemd 单元与 nginx 位置的**形状**，用占位符代替一切地址、路径与凭据。它不是生产授权：
切生产流量、动 `deploy/*`、接旧 Hub 都是单独的、需用户授权的步骤（[replacement-checklist.md](replacement-checklist.md)）。
Hub 自身**无鉴权**，只 loopback 绑定，放在**已鉴权的反代**后面（和节点侧一样）。

## 边界

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
| `SESSIONDOCK_HUB_NODES` | `hub-nodes.json` 注册表（0600），绝对路径；必填 |
| `SESSIONDOCK_HUB_CACHE_DIR` | 离线会话快照目录；缺省是注册表旁的 `hub-cache` |
| `SESSIONDOCK_HUB_NETWORKS` | 可注册的节点 CIDR（严格）；缺省 `127.0.0.0/8,::1/128`，私网段须显式配 |
| `SESSIONDOCK_WEB_DIR` | 前端快照（hub 模式），默认 `legacy-web` |
| `SESSIONDOCK_AUDIT_DIR` | 记 `hub.node.*.changed`（可选，0700 私有目录） |

## 注册（服务器端）

```sh
# 把节点凭据写进只读文件（Hub 与节点共享同一个 token 文件内容），再注册；地址是节点第二监听的私网 IP。
umask 077; printf '%s' "<NODE_TOKEN>" > <TOKEN_FILE>
sessiondock-hub register --name <MACHINE_NAME> --url http://<NODE_WG_IP>:<NODE_PORT> --token-file <TOKEN_FILE> [--color teal]
sessiondock-hub list           # 核对注册表（不含凭据）
sessiondock-hub remove <NODE_ID>
```

注册时 Hub 会打节点的 `/api/meta`，必须经过节点第二监听（带 token 与 `X-AgentHub-Protocol: 1`）、
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

- WebSocket（`/api/term/attach`）需要 `Upgrade`/`Connection` 透传；`proxy_buffering off` 保证 SSE
  `/api/watch` 与 NDJSON 搜索逐行到达浏览器。
- `client_max_body_size` 覆盖附件上限；读写超时给长连接（终端、SSE）留足。
- 反代把 Host 原样传给 Hub（Hub 的同源检查按 Host 核对 Origin）；Hub 只 loopback，不直接对外。

回退与切流步骤见 [replacement-checklist.md](replacement-checklist.md)。

经认证的反向代理前缀（同节点部署）：hub 进程也读 `SESSIONDOCK_PUBLIC_HOSTS`（例如 `203.0.113.177`），否则代理转发的 `Host` 会被 `hub_gate` 以 403 `local_only` 拒绝；`sessiondock-hub --check-config` 打印 `public_hosts=`。
