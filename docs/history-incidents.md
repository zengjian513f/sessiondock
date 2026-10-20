# 历史事故与验收记录

本文件专门保存特定版本的诊断经过、修复背景和验收回执。内容保留当时口径，
不作为当前规则、部署指令或当前产物的验证结果。当前合同见各来源文档。

## BUG-20261013-232935-441af3: repeated PTY upstream stalls

The Cygnus report was captured on build `20261013T223823185417Z`, commit
`afbe3f7c`, with a clean checkout. The complete manifest, browser state,
environment, 842 audit events and terminal capture were reviewed alongside
the lifecycle/conversation ledgers, the native Claude JSONL and the node/Hub
service journals. Times below are UTC on 2026-10-13.

- At 23:28:40 the first claim completed in 52 ms and the browser painted the
  initial grid within 167 ms. The first recorded input at 23:28:44.093
  (event 513, 27 bytes) had no corresponding node write receipt. This is the
  first observable input-path divergence, before the report dialog existed.
- Across the first four connections the browser recorded 42 inputs. Each
  received its initial screen but timed out its first bidirectional heartbeat
  after 10 seconds (events 533, 625, 697 and 792). None had an input-write
  receipt. Replacement claims completed in 49–72 ms, and ordinary HTTP/SSE
  traffic continued. The browser reported zero buffered WebSocket bytes at
  timeout; this is not a remote-delivery acknowledgement.
- The fifth connection opened at 23:29:27.254. Its first input receipt was
  two zero-byte resize frames (event 820), not the user's text. The terminal
  capture still showed an unsubmitted draft; the native history checkpoint
  stayed at 9277741 throughout the report window. PTY keystrokes do not use a
  SEND delivery ledger; the deployment had no `delivery/` directory, and its
  current conversation ledger and running lifecycle record were inspected.
- The subsequent daily audit shows new input on the fifth connection at
  23:29:37.784 and a node write window at 23:29:38.810 (39 bytes).
  At 23:29:51.028 native JSONL recorded a new user message. The same managed
  instance remained running; this recovery preceded this investigation and
  involved no repair or restart by the investigator.

The evidence identifies the browser-input → node-write gap and failed
bidirectional transport checks. It does not identify whether browser/network,
HTTPS proxy, Hub forwarding or node receive processing caused the gap.
The journals contain slow successful HTTP requests but no event establishing
a WebSocket failure cause. Header-button errors after opening the report are
occlusion by that dialog, not evidence that the terminal keyboard was blocked.
No speculative runtime change, input replay or production-data edit was made.

On 2026-10-14, `python3 tests/terminal_heartbeat_browser.py` passed in headless
Chromium with private synthetic fixtures: separately stalled downstream/upstream
sockets recovered, the host survived, and uncertain input was not replayed.
`python3 tests/terminal_rtt_browser.py --binary target/debug/sessiondock` also
passed: node and authenticated Hub paths each returned and painted all 85
keys (5 warmup, 40 individual, 40 grouped). Both runs used existing debug
binaries and the current frontend; they were not production-network tests.
These checks exercise recovery and input delivery, not the cause of this
intermittent production incident. The unresolved investigation remains in
[TODO](../TODO.md).

## BUG-20261015-135638-c5bbf6

For BUG-20261015-135638-c5bbf6, all 917 bundled events, the manifest, browser
state, environment, terminal capture, lifecycle binding and native JSONL
metadata were reviewed. The first observable diagnostic divergence is snapshot
587 after information-button click 583: it still reports the CSS-hidden terminal
as visible. Resource click 701 and information click 730 confirm the active
surface; no iframe wheel evidence existed. All 26 heartbeat replies matched,
HTTP response receipts were 200, native progress continued, and the lifecycle
binding was running. The four button errors followed report-modal occlusion.
No delivery directory existed and none was created.

The private Chromium baseline reproduced the information root's `overflow:hidden`:
2853px content in a 714px viewport stayed at scrollTop 0 after mouse wheel input.
The resource pane scrolled from 0 to 426 normally. The fix gives the information
root vertical scrolling; resource layout remains unchanged. The
[content scroll suite](../tests/content_scroll_browser.py) checks real wheels,
view changes, retry, geometry, themes/zoom and persisted audit records, including
an HTTP-served old-CSS fault to verify blocked-scroll diagnostics.

## BUG-20261015-154632-c5f7ce

BUG-20261015-154632-c5f7ce: all 3539 bundled audit rows, browser state,
environment and terminal capture were inspected, then correlated with the
origin's conversation/lifecycle ledgers and native Codex records. At 15:12:14 UTC
SEND succeeded and the CLI queue was observed. Native interruptions at 15:13:01
and 15:13:07 followed; the retained receipt is `interrupted`. The native user
record at 15:15:12 has edited wording, not the original submitted text. A later
answer cannot prove consumption of that original input. Keeping this explicit,
dismissible uncertainty follows the CLI-state contract; fuzzy text matching or
an automatic timeout would discard evidence. No production receipt was edited.

The report's first observable evidence gap is that none of its snapshots
identified the bubble or its state: the initial SEND also predates the bundled
window. Thus the old bundle alone cannot prove the precise label rendered at
report time. The new paired snapshots make browser/server disagreement directly
inspectable. The native Codex browser suite exercises interruption, reload,
report submission and persisted metadata without logging message text.

## BUG-20261019-095333-0cd031

BUG-20261019-095333-0cd031: the manifest, browser state, environment, all 1,218
bundled audit rows and terminal capture were correlated with the conversation
and lifecycle ledgers, native Codex JSONL, native runtime logs and the earlier
browser audit. No delivery directory existed. The current turn began at
08:08:44 UTC; its last output was at 08:10:55, with no native ending. At
08:10:54 the browser selected Stop on a different thread, confirmed at
08:10:56; the server recorded successful `/api/session/stop` at 08:11:03.
The shared daemon restarted at 08:11:04 and the affected TUI resumed its thread
at 08:11:10. At 08:11:38 CHECK observed ready/idle. This sequence and the old
process-stop implementation support collateral daemon termination, not a
manual interruption of the reported thread. The process ownership fix already
landed in `06a7c8f9`; the interruption predates that fix's deployment.

The first retained divergence is loss of the active native turn across that
daemon restart. The SDK reducer additionally discarded `turn/completed`'s
interrupted status and painted daemon idle as 就绪; its historical fallback
also omitted the conversation view's missing-ending inference. The isolated
[SDK interruption suite](../tests/codex_sdk_interrupted_browser.py) reproduces
the reducer fault with native RPC events, without restarting production.
The bridge now retains the last observed terminal outcome, and both views use
the same CLI observation fallback. No native records or production receipts
are rewritten. At report time the terminal's first snapshot already contained
the interruption; a later three-byte browser input cannot explain its origin,
and byte-count-only auditing does not identify those keys.

## BUG-20261020-033847-a2d757

2026-10-20 的 Agy SDK 报告已读取 manifest、browser-state、environment 和全部
980 条浏览器审计事件；没有 terminal.txt 或附件。浏览器 03:28 UTC 显示 SDK 活跃，
03:31:57 已不活跃，相关 HTTP 请求成功，没有用户停止事件；ResizeObserver 通知
不能解释执行中断。该 SDK 会话不走 PTY lifecycle 账本。

原生 transcript 与镜像最后保留 03:30:04 的“等待子代理与后台任务”回复，前一步
工具仍为 RUNNING；镜像 03:30:24 更新为 IDLE。持久输入账本保留消费证据
`observed`、`stop_requested=false`，最终为 `unknown`，错误明确记录服务重启后
投递结果未确认。systemd 记录 03:30:35 停止、03:30:37 启动，随后 03:31:25
再次停止、03:31:32 启动，与部署备份时间相符。

首个已证实的跨层问题是部署在尚未确认 SDK 输入结束时重启了节点服务。
Agy bridge 的 shutdown 会结束自己持有的 Child；实时快照不跨服务重启。
现有证据不能还原原生进程的精确退出时刻/退出码，也不能把原生 IDLE 等同于
审计任务和子代理汇总完成，因此不宣称已证实 CLI 自身崩溃或 OOM。

最初止血补丁的[部署检查](deployment.md)在 Linux staging 和 backup 前检查在途原生输入，
阻止已知活跃工作被正常发布打断。临时目录的隔离 run_push 验收覆盖 sending、
accepted、staging 后新增工作、终态/排队回执、缺失/损坏账本、Hub 和 dry-run；
拒绝路径没有 swap、restart 或 rollback。部署语法、文档链接、AGENTS 检查及
deploy dry-run 通过。本次仅改部署脚本和文档，未做生产重启或付费 CLI 重放。
后续架构修复将进程和原始通道移入[独立 SDK 宿主](sdkhost.md)，具体 SDK 协议
仍在节点服务。节点 shutdown 仅断开通道，重启复接原宿主，补读输出并恢复审批与
输入去重；明确停止仍终结原进程组。普通发布不替换宿主。旧进程内任务无法原地
迁移，部署检查继续保护它们，新宿主任务允许节点重启。原生后台任务完成后的
续跑语义与节点生命周期分开验收，见 [TODO](../TODO.md)。

### Incident BUG-20260927-020547-dd0ff9

The first observed transport failure began with the input-status request at
02:04:15.597 UTC, which exceeded its five-second deadline. Repeated status
checks then timed out in both the headers and body phases; message refreshes
and the report draft read also failed. The screenshot's raw `signal is aborted
without reason` came from the draft GET's local deadline. That GET previously
had no request audit, preventing precise correlation of its failure phase.

The native session completed at 01:02:53 UTC, matching the captured terminal
and displayed answer. No new delivery for the selected session was found.
Proxy access logs showed cancelled requests (499) and an empty 200 response
from the affected Windows client during the failure window, while other
clients continued receiving responses. Requests subsequently recovered and
the report was submitted successfully. These observations establish an
interrupted client request path, but do not distinguish browser, client network
or intermediary transport failure; they do not establish a node outage.

The fix makes the draft timeout explicit and adds missing transport evidence.
It does not claim to fix the unlocalized interruption. The Hub/node Chromium
acceptance test stalls the initial draft read through its real deadline, types
after failure, verifies automatic recovery without creating a worker, and
then submits once with exact attachment bytes.

## BUG-20261018-040613-165ffa

BUG-20261018-040613-165ffa: the browser sent the opening request 8 ms after
clicking and received its parsed body after 5902.6 ms, then rendered within
9 ms. The matching node trace recorded 5830 ms in the previously combined
read phase, 0 ms retiring echoes and 4 ms probing the prompt. The native turn
had completed before the click, and the lifecycle binding still named its
running instance; no delivery directory existed. A synthetic five-screenshot
Chromium open took approximately 0.4 seconds and did not reproduce the stall.
Follow-up phase logs captured 1519–2271 ms in snapshots with no admission delay.
The collector returned 71.8 MB / 103k boot-long connection records: liveness,
catalog acknowledgements and coordinator reads expanded them into a generic JSON
tree, cloned it and converted it again. Typed decoding on blocking workers and
bindings-only decoding for liveness remove those unnecessary allocations; the
coordinator still receives full evidence. In the synthetic 92 MB / 100k-record
Chromium regression, the old release peaked at 8776 MiB and cold opens took
468–1838 ms; the fixed release peaked at 285 MiB and took 34–343 ms.
This reproduces and fixes contention under collector load; the old combined
trace cannot retrospectively assign every millisecond of its 5830 ms stall.
See [collector regression](../tests/history_collector_browser.py).

## 同节点克隆原生验收

`session_clone_service_real.py`（已于 2026-10-06 随非浏览器测试删除）当时直接调用生产接口，在临时 home 使用 Codex 0.159.0、
`gpt-5.6-luna` low，创建真实分页分支、revert、归档兄弟和 code-mode 子代理。验证完整分页
内容、原生列表、当前 rollout、名称/置顶/项目关联，随后分别继续克隆分支、克隆子代理和
原分支，确认新旧互不追加。日常配置保持原样。该脚本只在显式直接调用或真实 CLI 验证时运行。

本节结果验证同节点 Codex；跨节点与其他 CLI 的独立证据见[迁移验收记录](#会话迁移交付验收2026-10-01)，
不能从此单项结果推导任意跨版本兼容。

## 跨节点迁移原生验收

`session_clone_service_real.py --production-peer <SSH 别名>`（已于 2026-10-06 随非浏览器测试删除）当时从真实 Codex 的可见孙分支经 Chromium
迁移整个五会话组；加 `--new-ids` 验证身份重写。Codex 0.159.2 / Luna low 实测包含分页 fork、
归档兄弟、revert 多代 rollout 和 code-mode 子代理，两种身份模式均通过目标原生列表、完整
分页历史、当前文件、名称/置顶/项目/归档元数据及分支/子代理续聊检查。目标 CLI 自行初始化
临时数据库，生产导入不调用测试 SQL 导入器；两端无关会话和日常默认配置保持不变，源端会话行
与历史清理完成。该验证不代表跨版本或所有工具/附件格式已经完成；运行回执使用下述合成浏览器验证。

## 会话迁移交付验收（2026-10-01）

## 交付验收

以下是 2026-10-01 收尾时核对的验收结果。浏览器套件使用临时合成会话和真实节点/Hub，
实际点击入口、选择操作、确认并打开目标历史；原生测试另使用隔离 home 和规定的低成本模型。
未用生产会话做测试。早期章节中的单项实验限制只描述当时样本，不替代本表后续证据。

本表保留当次验收回执。当前 `session_mixed_clone_browser.py` 的损坏暂存样例会在发布前
返回 `move_plan_stale` 并撤回旧计划，不再进入历史版本所测的发布后补偿分支；
现行套件覆盖范围见上面的[验证](session-move.md#验证)，发布后故障与恢复继续由相应 transfer 套件覆盖。

| 要求 | 已核对证据与结果 |
|---|---|
| 同机整组复制，Codex 分页 fork、多代 rollout、归档和 code-mode 子代理 | [session_clone_browser.py](../tests/session_clone_browser.py) 页面操作通过；`session_clone_service_real.py`（已于 2026-10-06 随非浏览器测试删除） 原生完整分页、列表、当前版本、元数据及新旧组分别续聊通过 |
| Claude/Grok 分支、子代理、跨父会话代理引用 | [session_files_clone_browser.py](../tests/session_files_clone_browser.py) 页面整组复制、历史打开、源文件不变、失败补偿和重启重试通过；最后一次包含 Grok 检查点依赖，共 9 项 PASS |
| 混合来源连通组、同 SID 不同来源、跨来源归属 | [session_mixed_clone_browser.py](../tests/session_mixed_clone_browser.py) 十四会话组、发布失败补偿和重启重试通过 |
| 跨机移动/复制 × 保留/改变身份 | [session_mixed_bundle_browser.py](../tests/session_mixed_bundle_browser.py) 在两台主机的独立临时存储完成四种组合；目标三种历史与关系、源端保留/清理、Codex 同 SID 两代 rollout 均通过 |
| 真实复杂会话迁移后恢复 | [session_native_transfer.py](../tests/session_native_transfer.py) 的原生迁移路径：Codex 五会话组、Claude/Grok 各三会话组，分别完成保留与重写身份；原生恢复与续聊、源端清理、无关会话保留、日常配置不变通过。Claude 为跨机往返后在最终目标续聊 |
| 目标已有副本、前缀扩展和失败恢复 | [session_prefix_browser.py](../tests/session_prefix_browser.py) 三种来源复制/移动及文件、原生行、显示设置恢复通过；[session_bundle_browser.py](../tests/session_bundle_browser.py) 相同目标复用与续写后重试不覆盖通过 |
| 停写、交接、清理与中断恢复 | [session_bundle_browser.py](../tests/session_bundle_browser.py) 覆盖源锁、部分清理、新外部引用、移回、旧回执、草稿别名、Hub/节点重启；保留和重写身份的移动均通过 |
| cwd 存在、外部附件及共享存储 | 同一双机套件 `--dependencies` 覆盖三种来源外部图片/结果/链接的缺失与异内容；独立本地存储和共享 NFS 探针实测通过；共享会话存储拒绝移动，不执行源端删除 |
| 原生 goal、附件关联、实时历史 | [session_clone_browser.py](../tests/session_clone_browser.py) 结构与映射验证通过；[session_goals_native_browser.py](../tests/session_goals_native_browser.py) 原生读取 goal/附件并独立修改克隆通过。实时连接不迁移 |
| Claude 原生文件回退 | [session_claude_rewind_browser.py](../tests/session_claude_rewind_browser.py) 原生 Edit、浏览器复制、新检查点回退及文件原字节恢复通过 |
| Grok 原生压缩检查点 | [session_grok_checkpoint_browser.py](../tests/session_grok_checkpoint_browser.py) Grok 1.0.44 原生压缩、浏览器复制、加载、回退和续聊读回标记通过；源文件和日常配置不变 |
| 目标 CLI 与动态工具提示 | [session_transfer_environment_browser.py](../tests/session_transfer_environment_browser.py) 缺 CLI、版本较旧/未知、切换目标后迟到响应及执行器未核验提示通过 |
| 步骤内部实时计数与进度条 | [session_large_manifest_browser.py](../tests/session_large_manifest_browser.py) 的 `--progress --peer` 模式，真实跨机移动、关联大历史与超 64 MiB 原生清单；记录页面更新间隔并核对源清理、目标原生数据 |
| 表格、机器与身份选择、禁用提示、窄屏及任务进度 | [session_clone_browser.py](../tests/session_clone_browser.py) 控件与 tooltip 通过；双机套件覆盖持久进度、刷新恢复和窄屏入口 |
| 交付 | 实现及测试已提交、推送；官方 `deploy --all` 已更新六个在线应用目标并通过健康检查，现有 ptyhost 保留。Cetus 离线跳过，未宣称该目标已更新 |

原生 Codex 证据覆盖 0.159.0 的克隆与 0.159.2 的复杂迁移；Grok 检查点证据为 1.0.44。
这些结果不构成所有 CLI 版本组合的兼容承诺。导入按实际原生 schema 核对，不创建或升级
CLI 数据库，不覆盖目标整库；不匹配时保留现场并报错。动态工具定义可迁移，但执行器、
认证及外部服务仍属于目标环境，页面会明确标为未核验。第一版 Linux、相同 CLI 根和 cwd
范围，以及不迁移进程/PTY、不复制工作目录的边界保持不变。

## Grok 文件集原生隔离实验

`session_files_grok_real.py`（已于 2026-10-06 随非浏览器测试删除）曾是显式运行的原生隔离实验：使用 `grok-4.6` low
创建父会话、fork 和子代理，将暂存输出实验性导入同一个临时 home，验证克隆后的父会话及
子代理按新 ID 续聊，再继续原组并核对两组互不追加。断言原生 chat 记录中的实际模型
`grok-4.6-build` 和 low 强度、原生关系以及日常配置不变。结果写入
`target/session-files-grok-report.json`；不代表生产发布或跨机验收通过。

## Agy 原生删除与索引回写

本次现场在原生删除成功后约 56 秒重新出现旧索引行，但数据库与正文仍不存在。
已确认异常从原生索引重新出现开始；未确定具体哪个 Agy 进程回写。
真实隔离双 CLI 保持另一会话运行并继续发送，观察 70 秒未重现该回写；常规浏览器
测试显式重放旧索引重新出现，验证刷新、服务重启、空壳删除与实际原生载体恢复。

真实 Agy 1.4.0、1.4.2 在私有 HOME、合成 loopback 模型网关中验证上述路径；常规浏览器测试
使用假 CLI，覆盖节点、Hub、批量、取消、身份不符与原生失败。更早版本、修改过的
快捷键和未知菜单尚未获得本次实机验收，不绕过原生确认。移动、克隆及整树删除仍不
支持，整组操作保留阻碍项，不能忽略 Agy 成员或仅迁移文本镜像。

## BUG-20260926-025909-1ee07a

BUG-20260926-025909-1ee07a exposed the missing OPEN-socket liveness check:
node output receipts continued while the browser had an approximately 14-second
receive gap, and browser inputs later arrived at the node in a burst with
negligible host-write latency. Other page traffic and native session work
continued. The bundle places the stall in the terminal transport but does not
identify which network/proxy hop caused it.

## BUG-20261015-030106-0869b4

BUG-20261015-030106-0869b4: browser history finished rendering at 03:00:43.152,
but the first input CHECK started at 03:00:45.791 and took only 160.6 ms.
The Hub recorded 4012 ms for the initial `/api/term/list`. The running lifecycle
record already had a confirmed native binding, and the captured terminal and
native JSONL showed ongoing work. The first mismatch was making this one
conversation wait for whole-fleet discovery. The available logs do not identify
which upstream caused that particular aggregation delay. No delivery directory
was present in that deployment; opening the conversation did not submit input.

## BUG-20261018-012536-a4c05c

BUG-20261018-012536-a4c05c: the earlier report's worker started at 01:14:59 UTC
and completed SEND at 01:15:04. Its native Codex history existed, but the
lifecycle receipt retained `binding: null`. At 01:25:03 the browser opened its
same-title pending row, still labelled “等待首条消息”, then at 01:25:08 opened
the separate native history. The rollout writer was the shared app-server,
outside the TUI ancestry required by process autobinding. Naming had already
confirmed the guarded TUI's full `/status` SID but discarded it. The
[binding recovery](lifecycle-binding.md) now preserves that identity and also
recovers old submitted workers whose status card has disappeared. No duplicate
worker launch or duplicate task SEND was found.

## BUG-20261013-154315-ac4fbd

BUG-20261013-154315-ac4fbd exposed a Grok registry entry whose PID was gone
and whose managed receipt was already Exited. The old unconditional hint made
cleanup repeatedly report running although there was no process to stop.

## BUG-20261018-050342-5d4448

The report's native event at 05:02:55 UTC contained the structured questions,
while the SDK item-to-message conversion discarded them; its terminal capture
showed one queued question. Browser audit had no SDK reply attempt.

## BUG-20261014-124811-809df3

BUG-20261014-124811-809df3：两次异常断联对应 Hub 在 12:40:50 / 12:47:44 UTC
达到 2 GiB cgroup 上限后被 OOM killer 终止；节点 PTY 与原生会话继续运行。
浏览器 WebSocket 1006、SSE 错误与 HTTP 502 同时出现，节点笼统的 1009 收帧错误
是上游骤断后的记录，不能据此认定用户发送了超大输入。另一次 4001 是显式接管。

## BUG-20261006-072955-02f968

BUG-20261006-072955-02f968 exposed this distinction: native command results
confirmed GPU workloads, while live collector samples contained their device
UUIDs but no session bindings. The jobs used short SSH launches of detached
workers; their launch clients had exited before diagnosis. Polling can miss
that connection evidence, and historical prose cannot safely recreate it.
This correction preserves the observed attribution gap in server aggregation;
it does not retroactively restore missing launch links. The event-driven connection
mechanism described in [process-links.md](process-links.md) closes the short-client capture gap for new observed
launches; it cannot recreate already-missing historical evidence.

## BUG-20261007-083559-acc010

BUG-20261007-083559-acc010 exposed a local CLI spawner that was present in
attribution but absent from `launch_chain`: child GPU samples existed, while
the parent's inclusive view omitted them. The process launch-chain correction fixes the
causal chain, preserving the sidebar's direct ownership scope. The accompanying
[historical launch audit](process-launch-audit.md) covers other program launches
and resource-release boundaries.

## BUG-20261008-120649-69fc59

BUG-20261008-120649-69fc59 的终端、原生历史和账本绑定一致，首个偏差是紧凑目录缺少
子会话时，浏览器把已绑定的启动重新投影成等待行，点击后却打开原生历史。

## Agy 编辑区与自定义状态栏识别

BUG-20261005-150140-776952：后台命令已在原生记录中进入 RUNNING，终端仍有空编辑区，
旧识别器因下方超过一行而使 CHECK 返回 409。任务栏现参与同一 CHECK/SEND 识别，
并令 `instance.busy` 为 true；任意正文、缺失分隔线及框外光标仍不恢复编辑区。
[菜单浏览器套件](../tests/cli_menus_browser.py) 用私有假 CLI 重放该布局，覆盖网页发送、
终端已有草稿、任务结束及菜单拒发。
BUG-20261006-041817-f7cf23：原生最后一条回复已 DONE、宿主仍 running，报告终端保留
完整空编辑框及默认页脚，其下多出自定义额度栏。浏览器恢复会话后的首个 CHECK
（2026-10-06 04:07:37 UTC）返回 409；旧识别器把所有多行页脚当作后台任务面板，
导致编辑区变成 unknown。现在从任务行结构识别面板，再独立定位默认页脚；其后的
自定义文本不参与输入正文或 busy 判断。同一浏览器套件覆盖叠加状态栏的 CHECK、
实际点击 SEND、忙碌/后台任务、原生草稿和框外光标；不修改原生会话或状态栏脚本。

## 前端清单来源记录

`grok-4.7 high headless 产出，人工审阅`
