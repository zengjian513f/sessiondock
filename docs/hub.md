# Hub：注册表、健康监控、节点客户端、命名空间、聚合、代理与 `sessiondock-hub`（第三十八批 H2、第三十九批 H3、第四十批 H4）

`crates/sessiondock/src/hub/` 是多机 Hub 的数据层：注册表（`registry.rs`）、
监控任务（`Monitor`）、Hub→节点 HTTP 客户端（`client.rs`）、节点身份/凭据文件
（`identity.rs`）、线上命名空间（`namespace.rs`）、五条读路由的聚合与三条分拆写
（`aggregate.rs`）以及对单台机器的代理（`proxy.rs`）。对照 Python `agenthub/hub.py` 的
`Registry`（73–492 行）与 `HubHandler.dispatch/selected/aggregate/search_aggregate/
bulk_*/purge_all/resolve/proxy`（518–1076 行），以及 `agenthub/federation.py`。数据层不绑定
监听、不读会话根；H4 的 `api/hub.rs` + `bin/sessiondock-hub.rs`（见「Hub 二进制」一节）把这些拼起来，
Hub 侧的两个数据构造函数是：

```rust
let registry = Arc::new(Registry::open(&nodes_file, networks, &cache_dir)?
    .with_public_payload(namespace::public_payload));   // 不装则缓存里是节点原样载荷
let client = Arc::new(Client::default());
let monitor = Monitor::spawn(registry.clone(), client.clone(), shutdown.clone());
```

## 节点身份与凭据（`identity.rs`）

- `PROTOCOL = 1`。
- `node_id(path)`：节点 id 文件。不存在则以 `O_EXCL` 0600 创建并写入 32 位小写十六进制
  （`secrets.token_hex(16)` 同形），已存在则只读（去首尾空白），内容不合法报
  `InvalidData`，绝不覆盖。
- `NodeToken::load(path)`：整文件去首尾空白后必须匹配 `[A-Za-z0-9._~+/=-]{32,256}`；
  `verify()` 用 `subtle::ConstantTimeEq` 常量时间比较（长度不同即不匹配，与
  `secrets.compare_digest` 一致）；`Debug` 输出脱敏。
- 与 Python 的 `_allowed()` 规则（带 `X-AgentHub-Protocol` 必须同时带正确 token 且协议为
  `"1"`）由 H1 的 `node_auth` 中间件实现，本模块只提供校验原语。

## 注册表（`registry.rs`）

**文件** `hub-nodes.json`：JSON 数组，元素 `{url, token, id, name, color?, enabled?}`，
2 空格缩进，0600，`.tmp` 同目录写入 + `fsync` + `rename`。`enabled` 缺省即启用，
只有明确 `false` 才停用。启动时校验每个 `url` 和 `id`（32 hex），不合法拒绝启动；
不存在的文件等于空表。

**注册**（服务器端操作，网页无此路由）`register(client, Registration{name,url,token,color,id})`：
名称去空白、1–80 字符；token 满足上面的文法；url `rstrip('/')` 后必须是
`http(s)://<字面 IP>[:端口]`（无路径/凭据/query/fragment，`/` 结尾允许，端口 1–65535，
HTTP 缺省 80、HTTPS 缺省 443，IPv6 用 `[…]`），且 IP 落在允许网络内（CIDR 列表，严格形式：主机位非零即拒绝；缺省与 Python 相同：
`127.0.0.0/8,::1/128,10.0.0.0/24`）。随后请求节点
`/api/meta`，必须 `mode:"local"`、`protocol:1`、`node_id` 32 hex，否则
`节点认证或协议检查失败，请先升级节点并配置凭据`；以 `node_id` 为主键替换同 id 条目
（追加到末尾），清掉该节点的缓存/健康/磁盘快照，落盘，`nudge()`。给了 `id` 而与节点
自报 id 不同 → `地址对应另一台机器，不能覆盖原节点身份`。颜色只能取
`NODE_PALETTE` 八色，空表示无色。

HTTPS 使用系统信任库校验证书链，并以注册 URL 的字面 IP 校验证书 IP SAN；升级为
WebSocket 后仍在同一 TLS 流上双向转发。HTTP 与 HTTPS 都继续受字面 IP 和网络白名单约束。

**显示属性** `update_display(nid, name?, color?, enabled?)`：名称非空且 ≤ 80 字符、不得与
其他机器同名；颜色同上，空串清除；`enabled=false` 视同不存在（清缓存与健康状态，监控
不再探它），`enabled=true` 恢复并重新加载磁盘快照、`nudge()`。`reorder(ids)` 必须是
全部机器（含停用）各一次的排列；`reorder_json` 附带 Python 的形状检查文案。
`remove(nid)` 删除条目、缓存、健康状态和快照。

**视图**：`all()`/`get()` 只含启用机器（聚合、监控、代理、uid 解析用）；`find()` 含停用
（设置页用）；`public()` = 启用机器 `{id,name,color} ∪ health`（无健康记录时
`online:null`）；`machines()` = 全部机器加 `enabled`，停用者 `online:null`。两者都不含
`url`/`token`。

**健康**（`Health`，键与 Python 的 dict 相同）：成功 →
`{online:true,last_seen,checked_at}`（其余字段清空）；失败（搜索除外）→
`strikes+1`、`failed_since` 取首次失败时刻、`error`/`error_code`/`failed_path`/`checked_at`；
之前在线且 `strikes < OFFLINE_STRIKES(2)` 时 `online` 仍为 `true`（一次慢答不置灰），
否则 `online:false` 且 `offline_since = 首次失败时刻`；从未在线的节点第一次失败即离线。
`state(nid)`、`offline(nid)`。搜索失败不改健康状态。

**缓存与快照**：`cache` 键 `(nid, path, urlencode(query))`，只存 200 且经
`public_payload` 改写后的载荷，上限 128 条（先进先出，`/api/search` 不缓存）。
`/api/sessions` 且 query 只含 `force`/`sig` 的完整答复另存为 `(nid,"/api/sessions","")`，
并按 `sig` 变化写 `hub-cache/<nid>.sessions.json`（`{stamp,data}`，0600，`.tmp`+rename，
在阻塞线程池写，失败忽略）。启动/重新启用时加载快照：缓存有了，健康记录为
`{online:null,last_seen:stamp}`，直到监控探过。`stale_payload(path, cached)`：给
`sessions`/`pending` 行加 `stale:true,last_seen`；`/api/term/list` 另置
`enabled:false,sources:{}`；无缓存则 `{}`。

**请求** `query(client,node,path,query,timeout,events)`：非 200 → `http_error`
（401/403/404/429/503 各有中文说明，其余 `节点返回错误响应（HTTP n）`）；200 非对象 →
`invalid_response`。`/api/sessions` 的 `{unchanged:true,sig}` 不改缓存。失败返回
`Fetched{data: stale_payload(...), failure: {node_id,name,error,error_code,last_seen}}`。
`check(node)` = 带缓存 `sig` 的条件请求；`check_all()` 最多 16 路并发；
`fetch()` 是聚合入口：已知离线的节点不等待，直接给失败记录（含 `offline_since`）和
过期缓存；空 query 的 `/api/sessions` 先条件探测，`unchanged` 则回最新缓存。
`recheck(node)` 用 `client.recheck`（5 s）探 `/api/live` 后 `nudge()`，返回是否在线。

**搜索** `/api/search`：始终以 `progress=1` 请求；`Content-Type` 非 NDJSON 时按 JSON 整读
（旧节点）。逐行读（行间空闲上限 `client.search_idle` = 60 s，总量 64 MiB），
`progress{done,total}` 与 `matches{results}`（先经 `public_payload`）送入
`mpsc::Sender<SearchEvent>`，`result{data}` 结束，`error` → `invalid_response`
（`节点搜索失败`），流提前结束 → `invalid_response`（`搜索响应不完整`），其他类型
（如 `heartbeat`）忽略。失败时已收到的 `matches` 行按 uid 去重后放进 `data.results`。
接收端关闭即中止读取。

**监控** `Monitor::spawn(registry, client, shutdown)`：启动即 `check_all`，随后每
`PROBE_INTERVAL`（10 s）或被 `nudge()` 唤醒时再跑；`spawn` 时丢弃此前积累的 nudge
（Python `start_monitor` 清事件）；`stop()`/shutdown 取消并等待。

## 节点 HTTP 客户端（`client.rs`）

**依赖决定：手写 HTTP/1.1，通过 `native-tls`/`tokio-native-tls` 支持 HTTPS，不引入
`hyper-util` legacy client。** Python 只用
`http.client`；Rust 端需要的也只是 GET/POST + 三种正文框架，而 `hyper-util` 的
client 会把连接池、解析器、`tower`、`tracing` 一并带进锁文件。手写版本在
`tokio::net::TcpStream` 上保证：

- 目标永远是注册表校验过的字面 `SocketAddr`：无 DNS、无 rebinding；
- 不跟随重定向（3xx 只是一个非 200 状态）；`Accept-Encoding: identity`；
- 每个 socket 操作都有空闲超时，与 Python 的 socket timeout 同义：连接 5 s；JSON 请求
  读 5 s；搜索流行间 60 s；代理 10 s、`/api/watch` 45 s（常量 `PROXY_TIMEOUT`/
  `WATCH_TIMEOUT` 供 H4 用）；
- 响应框架按 `http.client` 规则：1xx/204/304/HEAD 无正文；`Transfer-Encoding: chunked`；
  `Content-Length`（多值必须相同）；否则读到对端关闭；`100 Continue` 跳过；
- 状态行与每条响应头分别以 64 KiB 为界，响应头最多 100 条；不对全部响应头另设累计上限；
- JSON 正文上限 `JSON_LIMIT` = 64 MiB，超限是 `invalid_response`，绝不部分解析；
- 请求头/目标含 CR/LF 直接拒绝；每个请求都带 `X-AgentHub-Node-Token`、
  `X-AgentHub-Protocol: 1`、`Accept-Encoding: identity`，未指定 `Connection` 时加
  `Connection: close`（WebSocket 升级由调用方给 `Connection: Upgrade`）。

接口：`Client::json()`（一次 JSON 往返）、`Client::open()`（返回带流式 `Body` 的
`Response`：`read()`/`read_line()`/`read_to_end()`/`set_idle()`，`into_raw()` 交还
TCP/TLS 流与已预读字节供 101 升级双向转发）、`Client::connect()` + `Pending::send()`
（分块上传请求体后 `response()`）。失败类 `ClientError`：`timeout`、
`connection_refused`、`unreachable`、`connection_closed`、`invalid_response`、
`connection_failed`；`request_failure(error, status, timeout)` 给出与 `hub.py` 相同的
`(code, 中文原因)`，原因里永远没有 URL、token 或上游正文。

## 节点侧：第二监听与鉴权（H1，`config.rs` / `api/node_auth.rs` / `lib.rs` / `main.rs`）

节点不放宽 loopback 监听：`security.rs::local_only` 对任何 `x-agenthub-protocol` /
`x-agenthub-node-token` 头仍答 403 `hub_unsupported`（带真凭据也一样）。Hub 流量走
**第二监听**，四个变量必须同时给出，缺任何一个都是启动错误
（`SESSIONDOCK_NODE_BIND, SESSIONDOCK_NODE_TOKEN_FILE, SESSIONDOCK_NODE_ID_FILE and
SESSIONDOCK_NODE_PEERS must be set together (the node listener fails closed)`）：

| 变量 | 含义 | 校验（`Config::validate_node`，`--check-config` 同样执行且不写任何文件） |
| --- | --- | --- |
| `SESSIONDOCK_NODE_BIND` | 第二监听地址（WireGuard 接口 IP:端口） | 与 `SESSIONDOCK_BIND` 不同 |
| `SESSIONDOCK_NODE_TOKEN_FILE` | 节点凭据文件（Python `--node-token-file`） | 文件内容去首尾空白后为 `[A-Za-z0-9._~+/=-]{32,256}`（`NodeToken::load`） |
| `SESSIONDOCK_NODE_ID_FILE` | 节点身份文件（Python `--node-id-file`） | 已存在则必须是 32 位小写十六进制（只读），不存在则启动时由 `identity::node_id` 创建父目录并以 `O_EXCL` 0600 生成 |
| `SESSIONDOCK_NODE_PEERS` | 允许的来源网段（严格 CIDR，逗号分隔） | 主机位非零即拒绝；空表拒绝；没有缺省值 |

`--check-config` 额外打印 `node_bind` / `node_token_file` / `node_id_file` /
`node_peers`（未配置为 `(unset)`）。

第二监听复用同一份状态和 `/api` 路由（sessions/messages/media/files/term/SSE/WS），
不挂静态页（非 `/api` 路径一律 404 `not_found`），入口中间件 `node_auth` 按顺序检查，
任一不满足即 403、不进任何 handler：

1. TCP 对端地址（不看代理头；`::ffff:` 映射还原为 IPv4）∈ `SESSIONDOCK_NODE_PEERS`，否则
   `{"error":"forbidden","code":"node_peer_denied"}`；
2. `X-AgentHub-Protocol` 恰为 `1`，且 `X-AgentHub-Node-Token` 与凭据常量时间相等
   （`NodeToken::verify`），否则 `{"error":"node authentication required","code":"node_auth_required"}`
   （Python `/api/meta` 的原文）。

之后仍走两端共享的 `security::api_policy`（`sec-fetch-site: cross-site` 403、Origin 必须等于
Host、URI/正文上限、响应头；`debug_run` 是列表视图选择器，见 read-model.md），只是不做 loopback Host 检查——Hub 以私网 IP
访问节点。`/api/meta` 在两个监听上都报 `protocol: 1` 与文件里的 `node_id`（未配置身份时仍是
`protocol: 0, node_id: null`）；loopback 上的 `/api/nodes` 变为 Python 节点模式的
`{mode:"local", nodes:[{id, name: hostname, online:true}]}`；`capabilities.hub` 恒为 false
（节点不是 Hub）。`main.rs` 先绑定两个 socket 再开始服务：节点监听绑定失败即启动失败，不会
退化成只有 loopback 的服务；两个监听共用同一个关停信号。

验证：`cargo test -p sessiondock --lib -- config:: api::node_auth`、
`cargo test -p sessiondock --test node_auth --locked`、`python3 tests/node_auth_suite.py`
（两个监听都绑 127.0.0.1，`PEERS=127.0.0.0/8` 时三缺一 / 错 token 403、正确头 `/api/sessions`
与 loopback 列表一致、静态页 404，`PEERS=10.100.100.0/24` 时同一请求 403 `node_peer_denied`，
三缺一的环境拒绝启动）、`tests/check_config_suite.py` 的 `node_*` 用例，
`tests/security_suite.py` / `tests/meta_capabilities_suite.py` 的新增断言。

## 线上命名空间（`namespace.rs`）

节点交出去的每个引用在到达浏览器前带上节点 id，回到节点前再去掉；原生 CLI id、
消息文本、工具参数和文件内容永远不动。形状：会话 uid `<source>:<nid>~<local tail>`；
终端名、回收站条目 id、outbox epoch `<nid>~<local>`；媒体 `src`
`/api/nodes/<nid>/api/media/…`。

- `qualify(node, value, uid)` / `split(value, uid)`：`federation.qualify/split` 逐字。
  空值原样返回；uid 缺 `<source>:` → `invalid session reference`（qualify）/
  `missing session source`（split）；`split` 要求节点段是 32 hex 且本地段非空，否则
  `missing or invalid machine reference`。错误类型 `NamespaceError`，`message()` 即
  Python 的 `ValueError` 文案，H4 映射为 400 `{"error": …}`。
- `public_payload(data, node, path)`（`registry::PublicPayload` 形状）：
  `federation.public_payload` 的 `walk` + 按路径装饰。`walk` 只改写键
  `uid/from_uid/to_uid/continued_in`（非空字符串）、`src`（以 `/api/media/` 开头）、
  `epoch`（字符串），遇到 `data/content/input/arguments/raw/resolved` 整棵原样复制；
  顶层非对象只走 `walk`。路径装饰（`decorate_row`：写入 `node_id/node_name`，
  `terminal` 时非空 `name` 加前缀，`trash` 时非空 `id` 加前缀）：
  `/api/sessions`、`/api/search` 的 `sessions`（无则 `results`）行；`/api/live` 总是重写
  `uids/tmux_uids/started_at` 三键（缺则补空）；`/api/term/list` 的 `sessions/pending`
  行（terminal）；`/api/term/create|takeover|new-status` 顶层（terminal）+ 真值
  `session`；`/api/bug-report` 真值 `worker`（terminal）；`/api/trash` 的 `items`
  （trash）；`/api/messages/*`、`/api/watch` 的真值 `meta`。
- **与 Python 的唯一差异**：Python 遇到无法加前缀的 uid（缺 `<source>:`）抛
  `ValueError`，整个节点答复按 `invalid_response` 失败；注册表钩子是不可失败的函数，
  `public_payload` 把这样的引用原样保留、其余照常改写。要 Python 的行为用
  `try_public_payload`（返回 `Result`）。非字符串的终端名 Python 会 `str()` 后拼接，
  Rust 不动；真实节点只发字符串。
- 奇偶校验：`tests/hub_namespace_parity.py --python-source <pyhead>` 在进程内
  `importlib` 加载 `agenthub/federation.py`，把固定语料（51 例：每个改写键、嵌套但
  不透明的子树、媒体 src、epoch、终端名、回收站 id、每条装饰路径、顶层标量/数组、
  非 ASCII，含 5 例 Python 抛错）跑过 oracle 写成
  `tests/fixtures/hub_namespace_cases.json`（`--write`），默认模式重新生成并比对已提交
  的夹具，漂移即失败。`cargo test -p sessiondock --test hub_namespace` 回放夹具：
  成功例要求 `try_public_payload` 与 `public_payload` 都与 oracle 相等，抛错例要求
  `try_public_payload` 以相同文案拒绝；打印 `DIFF 0`。

## 聚合（`aggregate.rs`）

输入统一是浏览器 query 的键值对 `&[(String, String)]`（`Params`，wire 顺序）；输出
`serde_json::Value`；被拒绝的输入是 `AggregateError`（`message()` = Python
`ValueError` 文案 → 400 `{"error": …}`）。

**选择**：`selected(registry, query)`：无 `nodes` → 全部启用机器；`nodes=a,b`（空项忽略，
`nodes=` 选空）按注册表顺序过滤，含未注册/停用 id → `筛选包含未注册的机器`。
`upstream(query)` 去掉 `nodes/sig/progress` 后按键首次出现分组（同
`urlencode(parse_qs(...), doseq=True)`），原样转给节点。

**并发**：每条读路由对所选机器同时 `Registry::fetch`（最多 `FAN_OUT` = 16 路，答复按
选择顺序），已知离线的机器不等待（注册表给失败记录和过期缓存）。每个答复的
`failure` 记录进 `errors`，`partial = errors 非空`，`nodes = registry.public()`（取完
答复后的健康状态）。

- `sessions(registry, client, query)`：行 = 各机 `sessions` 拼接，按
  `(updated, uid)` 降序（Python `reverse=True`，注意与本地列表的 uid 升序不同）；
  `truncated = any`、`truncated_nodes`、`total_pool = sum`。`sig` = 把 `nodes` 行缩成
  `{id,name,online}` 后按排序键 JSON 求 sha256 前 24 hex；与 `?sig=` 相同则只回
  `{unchanged:true, sig, nodes, errors}`，否则附 `sig`、`built_at`。sig 只和本实现
  自己产生的值比较，不与 Python 的字节相同（切换时浏览器多刷一次整表）。
- `search(registry, client, query)`：`progress` ≠ `1` 的 JSON 版，形状同上但无 sig。
- `live`：`uids/tmux_uids` 只拼接答复成功的机器；`started_at` 合并。
- `term_list`：`enabled = any`（含过期缓存里的 `enabled:false`）、`home:""`、
  `sessions/pending` 全部拼接（含 stale 行）、`capabilities[nid] = {enabled: 真值且未失败,
  unavailable_reason: 失败文案或节点的, sources, home, backend, backends（失败为 []）}`、
  `sources` 是成功机器的并集（`prev or available`：已为真不覆盖）。
- `trash`：`items` 拼接、`size` 求和、`dir = "所选机器的本地回收站"`。
- `search_stream(Arc<Registry>, Arc<Client>, query) -> Result<impl Stream<Item = String>>`
  （`progress=1`，`progress_requested(query)` 判定）：每项一行 NDJSON（`serde_json`
  紧凑形式 + `\n`）。先发一条 `progress`（每机 `{id,name,done:0,total:null,state:
  "preparing"}`，已知离线的 `total:0,state:"offline"`），然后每台机器一个任务
  `fetch`（`Semaphore(16)`），进度/命中经 `mpsc(64)` 汇总：节点 `progress` →
  该机 `{done,total,state:"scanning"}` 并重发汇总 `progress{done=Σdone,
  total=Σ(total or 0), total_known=all(total≠null), nodes}`；节点 `matches` → 原样
  `matches{results}`；该机结束 → 失败时 `state = offline|error`（离线且 total 为
  null 补 0），成功时 `total` 取已知或 `total_pool`，`state = limited|done`，`done =
  scanned`（缺则截断时保持、否则 = total），重发 `progress`，`data.results` 真值再发
  一条 `matches`；1 s 无事件发 `{"type":"heartbeat"}`；最后 `result{data}` = JSON
  `search` 体。选择错误在流建立前返回（H4 仍可答 400）。流被丢弃（浏览器断开）即
  中止所有节点任务。
- `delete(registry, client, body)`：`uids` 先 `split` 分组（首次出现顺序），空 →
  `没有选中任何会话`；`force` 保持布尔值并逐机顺序转发
  `POST /api/sessions/delete {uids,force}`（`client.timeout`），
  200 的答复经 `public_payload` 后并入 `deleted/errors`；机器未注册/非 200/连接失败
  → 该机每个 uid 一条 `{uid: 重新限定, error: "机器请求失败，请核对结果"}`。
- `fork_visibility`：`visible` 必须是布尔（`需要布尔值 visible`），空 →
  `没有选中任何父会话`，其余同上（`updated`）。
- `purge_all(registry, client, query)`：对 `selected` 的每台机器顺序
  `POST /api/trash/purge {all:true}`，`removed/freed` 求和，节点的 `errors` 加
  `"<机器名>: "` 前缀，失败 `"<机器名>: 请求失败，请核对结果"`。

## Hub 二进制与 HTTP 面（H4，`api/hub.rs` / `bin/sessiondock-hub.rs` / `hub_config.rs` / `assets.rs`）

Hub 是**独立二进制** `sessiondock-hub`（同 crate，`src/bin/sessiondock-hub.rs`），只 loopback，
放在已鉴权反代后，不共享节点的任何私有目录（不读会话根、host、账本、state）。配置在新模块
`hub_config::HubConfig`（从环境变量读、校验，`--check-config` 只跑校验并打印 `hub_bind` /
`hub_nodes` / `hub_cache_dir` / `hub_networks` / `web_dir` / `audit_dir`，什么都不开、不绑、不注册）：

| 变量 | 含义 | 校验 |
| --- | --- | --- |
| `SESSIONDOCK_HUB_BIND` | Hub 监听（默认 `127.0.0.1:8742`） | 必须是 loopback 地址（Hub 自身无鉴权） |
| `SESSIONDOCK_HUB_NODES` | `hub-nodes.json` 注册表 | 缺省 `~/.local/share/sessiondock/hub-nodes.json`；不存在即空注册表，首次保存递归创建父目录 |
| `SESSIONDOCK_HUB_CACHE_DIR` | 离线会话快照目录 | 缺省是注册表旁的 `hub-cache`；保存快照时递归创建，读写失败不影响在线 Hub |
| `SESSIONDOCK_HUB_NETWORKS` | 可注册的节点 IP/CIDR（严格） | 主机位非零即拒；缺省 `127.0.0.0/8,::1/128,10.0.0.0/24` |
| `SESSIONDOCK_WEB_DIR` | 前端快照（hub 模式） | 必须是存在的目录 |
| `SESSIONDOCK_AUDIT_DIR` | `hub.node.*.changed` 记录（可选） | 未配置则不记显示/顺序变更 |

注册表、缓存与审计路径使用普通文件系统路径语义：相对路径、`..`、符号链接以及目录重叠都不在
配置阶段额外拒绝。注册表和快照仍以 `0600` 模式创建临时文件后替换目标；已有文件的权限不作为
启动门槛，实际文件读写错误按相应操作返回。该行为对应 Python `Path` / `mkdir(parents=True)` /
`os.open(..., 0o600)` 的组合。

**注册是服务器端操作**（Python 的 `Registry` 类，网页无注册路由）：子命令
`sessiondock-hub register --name --url --token-file [--color] [--id]`（token 从文件读，绝不进
进程列表）、`remove <nid>`、`list`。`register` 调 `Registry::register`，注册时打节点的
`/api/meta` 必须经过节点的**第二监听**（带 `X-AgentHub-Node-Token` 与 `X-AgentHub-Protocol: 1`）。

**页面（`assets.rs` 的 `Mode::Hub`）**：同一份 legacy-web 快照，`__AGENTHUB_MODE__=hub`、
hostname `SessionDock`、能力声明 `hub_capabilities()`（`backend:rust, hub:true,
storage_namespace:"sessiondock.hub.", history_pages, media_continuation`——每机能力
terminal/outbox/files/trash/live/audit 都不声明，页面按每台机器的 `/api/term/list.capabilities`
逐条降级，和 Python 的 hub 页面一样；`media_lazy` 有意不声明，否则页面会在 hub 判断前把
`/api/nodes/<nid>/api/media/…` 源置空）。Hub 页面的 storage 命名空间是
`sessiondock.hub.<location.pathname>.`——快照不知道挂载路径，注入一段脚本在主题脚本和
capabilities.js 读取前把路径补进 `storage_namespace`（对应 Python 按 `location.pathname` 区分
不同挂载点的 hub）。

**分发（`api/hub.rs::dispatch`，对照 `HubHandler.dispatch`）**：一个网关 + 一个分发器。网关
（`hub_gate`）在任何 handler 前：loopback Host 或 `SESSIONDOCK_PUBLIC_HOSTS` 列出的反向代理权威（与节点同一规则 `security::host_allowed`；否则 403 `local_only`）、拒绝任何 hub 协议/token
头（403 `hub_unsupported`）、`/api/` 的跨站（403 `cross_site`）与不同源写（403，文案
`cross-origin write rejected`）、HTTP 请求行（method、URI、version）≤ 64 KiB；响应加
`nosniff`/`Referrer-Policy`/API `no-store`。
分发器按顺序：非 `/api/` 的 GET → hub 模式静态页；`GET /api/meta` →
`{mode:"hub",protocol:1,build,hostname:"SessionDock",capabilities}`；`GET /api/nodes` →
`{mode:"hub",nodes:public(),machines:machines()}`；`POST /api/nodes/order` → `set_order`；
`POST /api/nodes/{32hex}/display` → `set_display`（名称/配色/启用，改动记
`hub.node.display.changed`/`order.changed` 审计）；`/api/nodes/{32hex}/api/*` 显式节点直连；
`GET /api/session/file` 且 `Accept: text/html` → 303 到 `../../../../../file.html?…`
（`proxy::file_navigation`）；五条聚合读（非显式节点）走 `aggregate::*`，`/api/search` 且
`progress=1` → 200 `application/x-ndjson; charset=utf-8` + `Cache-Control:no-store` +
`X-Accel-Buffering:no` + `Connection:close`，正文是 `search_stream` 每行一条 NDJSON；
`POST /api/sessions/delete`、`/api/sessions/fork-visibility`、`/api/trash/purge{all:true}`、
`/api/audit/browser`（`proxy::group_browser_audit` 按事件 uid 分组，pending uid 落到该 page 最近
一次解析出的机器、uid 置空，`_page_nodes` LRU 512）分拆到各机；其余一律经 `proxy::resolve` 解出
**唯一**机器再 `proxy::proxy`。

**resolve / proxy（`proxy.rs`，对照 `HubHandler.resolve` 760-809 / `proxy` 951-1076）**：
`resolve` 从显式路由、路径（`/api/messages/<global>`、`DELETE /api/session/<global>`；Rust 的
`/page`、`/media-page` 后缀保留）、query（`uid`，`name` 仅 term 路由，`node`）、body（`uid`、
`name`、`_node`、`terminal_name`、`id`、`media[].src`）收集机器 id，去限定成本地引用；多于一个 →
400 `操作必须明确指定同一台机器`，附件来自另一台 → 400 `附件来自另一台机器`。机器未注册 → 404；
离线且 `recheck` 仍失败 → 503 `{error,node_offline,node_id,offline_since}`；
`send/outbox/retry/term/send/term/create` 校 `body._build == 前端 build`，否则 409
`{reload:true,build}`。`proxy` 带节点头转发，透传 `Content-Type,X-AgentHub-Page,X-AgentHub-Trace,
X-AgentHub-Build,Range`、加 `X-Real-IP`（`_display_ip`：`X-Real-IP`→首个 `X-Forwarded-For`→TCP
对端）：JSON 经 `public_payload` 改写并带 `X-AgentHub-Decoded-Length`；`text/event-stream` 按
`data:` 行改写（45 s 读超时）；WebSocket `/api/term/attach` 101 后用 `hyper::upgrade::on` +
`hyper_util::rt::TokioIo` 拿到浏览器连接，和 H2 客户端 `Body::into_raw` 交还的节点 TCP/TLS 流做
`tokio::io::copy_bidirectional` 裸转发（预读字节先发）；其它正文流式透传并保留
`Content-Length/Content-Disposition/X-Content-Type-Options/Cache-Control/CSP/Content-Range/
Accept-Ranges`；附件上传（`attachment`/`files/upload`）按 `Content-Length` 分块转发（≤
`ATTACHMENT_MAX_BYTES` = 512 MiB），超限先拒。

**与 Python 的差异**：WebSocket 用裸 TCP 双向拷贝（Python 做法，101 后不解帧），HTTP 客户端手写、
仅字面 IP、不跟随重定向（见「节点 HTTP 客户端」）；这些沿用 H2 的既有决定，H4 不引入新差异。

## 验证

- `cargo test -p sessiondock --lib hub:: --locked`（26 例）：identity 4 例、client 10 例
  （请求头、POST 正文、Content-Length/chunked/读到关闭/204、NDJSON 逐行与 100 跳过、超限、
  不跟重定向、超时/拒绝/中断/坏状态行/非 JSON、失败文案、101 交还 socket、坏请求拒绝）、
  registry 12 例（CIDR、URL 校验、urlencode、stale 形状、注册与主键、显示/停用/顺序/
  持久化、strikes 与恢复、首次失败即离线与失败类别、快照 0600/重启/过期行、条件 sig/
  缓存上界、搜索事件/失败/保留命中、监控轮询与 nudge）。
- `cargo test -p sessiondock --test hub_registry --locked`（10 例）：对
  `tests/hub_fake_node.py`（`hub_fixture.NodeHandler` 的 stdlib 移植，由 `python3` 启动，
  `/__control`/`/__state` 控制离线、慢答、删除、凭据、搜索脚本、超大响应）复刻
  `test_hub.py` 的注册表用例：注册与公开行、条件 sig/force/列表变化、快照跨重启与过期行、
  离线节点不等待 + recheck 恢复、安全失败文案（503/403）与恢复清空、一次慢答不置灰、
  首次失败即离线、搜索流进度跨空闲超时 + JSON 兼容、搜索失败不改健康并保留命中、
  64 MiB 上限。
- `cargo test -p sessiondock --lib hub::namespace --locked`（10 例）：qualify/split
  往返与错误文案、resolve-files 不改引用名、载荷边界（input/text/native id/media src）、
  continued_in、live 三键、终端名/回收站 id 只在对应路径加前缀、epoch/src 不进不透明
  子树、严格/宽松两种改写、顶层标量与数组。
- `cargo test -p sessiondock --lib hub::aggregate --locked`（11 例，不联网）：机器
  筛选与 400、upstream 分组、行排序、求和、sig 只跟行与 `{id,name,online}` 走、
  `unchanged` 形状与键序、live 只算成功机器、term/list capabilities 与 sources 并集、
  trash 合计、progress 行、bulk uid 分组与错误文案。
- `tests/hub_namespace_parity.py --python-source <pyhead>` + `cargo test -p sessiondock
  --test hub_namespace --locked`：51 例 0 DIFF（见上）。
- `cargo test -p sessiondock --test hub_aggregate --locked`（13 例）：对两台
  `tests/hub_fake_node.py` 复刻 `test_hub.py` 的聚合用例：跨机 uid 唯一与 sig/unchanged/
  变化、`nodes` 筛选与未注册 400、离线机器 partial + 过期缓存行且不等待、term/list
  capabilities（每机 backend/backends、sources 并集、离线机器的失败文案与空 backends、
  stale 终端行）、trash 合计与筛选、搜索流的 progress/matches/heartbeat/result 顺序、
  等齐 totals 与截断扫描（3/11、limited 2/10）、搜索失败不改健康且保留已流出的命中、
  空选择/离线机器/JSON 旧节点、丢弃流即取消扫描、bulk delete / fork-visibility 按机
  分组与逐 uid 报错、purge_all 求和与前缀。
- `cargo test -p sessiondock --lib -- hub::proxy hub_config`（proxy 9 例 + hub_config 2 例）：
  parse_qs/quote/unquote 往返、显式/display 路由、resolve 唯一节点与去限定、mixed/foreign/missing
  拒绝、`_node`/`node` 与显式路由、file_navigation 三种放行/拦截、`_page_nodes` LRU 与 browser
  audit 分组（含 pending uid 落最近机器）、上游头/`display_ip`/SSE `data:` 改写、ClientError 映射；
  hub_config loopback bind、前端目录检查，以及注册表/缓存/审计的 Python 路径语义。
- `cargo test -p sessiondock --test hub_http --locked`（8 例）：对两台 `tests/hub_fake_node.py`
  复刻 `test_hub.py` 的 dispatch/proxy 用例——hub 模式页面 + 命名空间脚本、`/api/meta`、`/api/nodes`
  无凭据、网关（Host/hub 头/跨源/无注册路由）、resolve 唯一节点与 400/404、`_build` 409、
  离线 503 形状 + 过期行 + recheck 恢复、messages/media 改写与显式后端、SSE `data:` 改写、
  分块附件上传（含 bug-report `?node=`、越限拒绝）、display/order 校验与审计记录、bulk
  delete/restore/audit 按机分组、NDJSON 搜索、WebSocket 双向裸转发（含网关仍生效）。
- `tests/hub_http_suite.py`（Python 全程走 HTTP）：`--check-config` 与 fail-closed、register/list/remove
  子命令（注册表私有、凭据不入输出）、上面全部用例含 NDJSON 搜索进度。
- `tests/hub_browser.py`（Playwright，≤300 行，Chromium
  `$HOME/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome`）：hub 页对三台假节点——机器筛选
  单/多选、按机器分层（同原生 id 不跨机嵌套）、NDJSON 搜索进度 + 单机失败、经代理打开同一原生 id
  的两台机器（媒体 + SSE）、设置页停用/启用/拖动/键盘换序/改名，`sessiondock.hub.<path>.` 前缀。
- **真实根只读验收**（不进 sweep）：起一台真实 Rust 节点（debug 二进制指向真实读根、第二监听在
  loopback、`SESSIONDOCK_NODE_PEERS=127.0.0.0/8`）+ 一台假节点，`sessiondock-hub register` 注册两台，
  hub 在 loopback 端口起、`SESSIONDOCK_WEB_DIR=<worktree>/legacy-web`，Playwright 列出真实会话
  （≈800 行，`<source>:<nid>~…` 命名空间）、经代理打开最新一条真实会话、搜索流出进度、设置页可用；
  结束核对读根 mtime/size 未变。实测 810 会话（809 真实节点）、冷列表 2.8 s、打开最新会话 1.1 s、
  搜索 0.3 s、无控制台错误。脚本用最便宜配置、只读、末尾自删（本 WP 未创建临时会话）。

## 接口（`api/hub.rs` 调用）

```rust
// hub::namespace
pub fn qualify(node: &str, value: &str, uid: bool) -> Result<String, NamespaceError>;
pub fn split(value: &str, uid: bool) -> Result<(String, String), NamespaceError>;
pub fn public_payload(data: Value, node: &Node, path: &str) -> Value;          // PublicPayload 形状
pub fn try_public_payload(data: Value, node: &Node, path: &str) -> Result<Value, NamespaceError>;
pub fn decorate_row(row: &mut Map<String, Value>, node: &Node, terminal: bool, trash: bool) -> Result<(), NamespaceError>;
pub fn decorate_rows(rows: &mut [Value], node: &Node, terminal: bool, trash: bool) -> Result<(), NamespaceError>;
impl NamespaceError { pub fn message(&self) -> &'static str }                  // 400 文案

// hub::aggregate
pub type Params = [(String, String)];
pub struct AggregateError(pub String);  impl AggregateError { pub fn message(&self) -> &str }
pub const FAN_OUT: usize = 16;  pub const HEARTBEAT: Duration = 1 s;  pub const TRASH_DIR: &str;
pub fn first<'a>(query: &'a Params, key: &str) -> Option<&'a str>;
pub fn progress_requested(query: &Params) -> bool;
pub fn selected(registry: &Registry, query: &Params) -> Result<Vec<Node>, AggregateError>;
pub fn upstream(query: &Params) -> Vec<(String, String)>;
pub async fn sessions(registry: &Registry, client: &Client, query: &Params) -> Result<Value, AggregateError>;
pub async fn search(registry: &Registry, client: &Client, query: &Params) -> Result<Value, AggregateError>;
pub async fn live(registry: &Registry, client: &Client, query: &Params) -> Result<Value, AggregateError>;
pub async fn term_list(registry: &Registry, client: &Client, query: &Params) -> Result<Value, AggregateError>;
pub async fn trash(registry: &Registry, client: &Client, query: &Params) -> Result<Value, AggregateError>;
pub fn search_stream(registry: Arc<Registry>, client: Arc<Client>, query: &Params)
    -> Result<impl Stream<Item = String> + Send + 'static, AggregateError>;   // 每项一行 NDJSON
pub async fn delete(registry: &Registry, client: &Client, body: &Value) -> Result<Value, AggregateError>;
pub async fn fork_visibility(registry: &Registry, client: &Client, body: &Value) -> Result<Value, AggregateError>;
pub async fn purge_all(registry: &Registry, client: &Client, query: &Params) -> Result<Value, AggregateError>;

// hub::proxy（H4 内部；resolve/proxy/浏览器审计/页面导航）
pub fn resolve(explicit: Option<&str>, method: &str, path: &str, query: Query, body: Option<Map<String,Value>>)
    -> Result<Resolved, ProxyError>;                                          // Resolved{nid,path,query,body}
pub async fn proxy(client: &Client, target: &Target, node: &Node, forward: Forward<'_>) -> Result<Response, ProxyError>;
pub fn group_browser_audit(body: &Map<String,Value>, page_nodes: &PageNodes) -> Vec<AuditGroup>;
pub fn file_navigation(accept: &str, query: &Query, node: Option<&str>) -> Result<Option<String>, ProxyError>;

// 二进制入口（bin/sessiondock-hub.rs 用）
pub fn hub_app(config: &HubConfig, shutdown: CancellationToken) -> io::Result<HubApp>;    // router+registry+client+monitor
pub fn open_registry(config: &HubConfig) -> io::Result<Registry>;                          // register/remove/list 子命令用
```

分发（`api/hub.rs::dispatch`，对照 `HubHandler.dispatch`）：GET 且非显式节点、路径在五条之内 →
`selected` 出错答 400；`/api/search` 且 `progress_requested` → 200
`application/x-ndjson; charset=utf-8` + `Cache-Control: no-store` + `X-Accel-Buffering: no`
+ `Connection: close`，正文 `Body::from_stream(search_stream(...))`；否则对应函数的
JSON。`POST /api/sessions/delete`、`/api/sessions/fork-visibility` 走 `delete`/
`fork_visibility`（body 是已解析的对象）；`POST /api/trash/purge` 且 `body.all` 真值走
`purge_all(query)`；其余经 `resolve` → `proxy`（见「Hub 二进制与 HTTP 面」一节）。
