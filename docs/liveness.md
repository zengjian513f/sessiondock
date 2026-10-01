# External CLI liveness: the `/proc` scan

The process scan sits next to the managed
host observations ([processes.md](processes.md)). On platforms with
native process discovery, `/api/live` exposes the complete set the
frontend expects (`capabilities.live: true`).

## Scope

- `SESSIONDOCK_PROC_ROOT=<dir>` (default `/proc`) is the process table to read;
  tests point it at a synthetic tree.
- Grok's active-sessions file defaults to
  `~/.grok/active_sessions.json`; `SESSIONDOCK_GROK_ACTIVE=<file>` provides a
  test override. Entries name sessions live without a pid, stale ones included.
- On a target without native process discovery, `/api/live` reports
  `scan: {status: "unsupported_platform"}`, `capabilities.live` stays false and
  managed observations remain available. External checks return no PID evidence
  and therefore never signal a process. A Windows provider may
  be added independently of the HTTP and lifecycle contract.

The scan is read-only: it lists the table, reads `cmdline` of every process,
and `environ`, `cwd` and `fd/*` links of the processes whose command line
mentions `claude`, `codex` or `grok` (plus the `cwd` of `opencode` processes);
`stat` lines are read lazily for ancestry walks. It never signals, writes, follows a link outside the tree, or elevates
privileges (another user's `environ`/`fd` are unreadable and simply skipped;
`cmdline` is world-readable).

Results are cached for 3 s counted from the moment a scan completes (a slow
scan must not expire its own result), refreshes are single-flight (waiters
reuse the one in progress) and `?force=1` bypasses the TTL while still
serializing. On this machine (256 cores, ~3000 processes,
~180 keyword matches) one debug-build scan takes 60–70 ms.

## Rules kept from `live.py` (commit 16cc89c)

Per matching process:

1. `--session-id`/`--resume` UUIDs on the command line are the process's
   identity (`sids[sid] += pid`).
2. A bare `claude` (main process, no id anywhere) is remembered with its
   resolved cwd and start time; it is paired later with a Claude session whose
   `cwd` resolves to the same directory and whose `created` lies within
   −5 s … +30 s of the start.
3. `CLAUDE_CODE_SESSION_ID` / `CODEX_COMPANION_SESSION_ID` / `GROK_SESSION_ID`
   in the environment count only when the process owns them: a main process
   that named an id on its command line ignores an inherited different id; a
   helper is attributed to its nearest CLI ancestor by `comm` (12 levels), and
   an orphan without one (a `setsid`/`nohup` script left after the CLI exited)
   counts for nothing; a Grok owner accepts only `GROK_SESSION_ID`; a main
   process never takes another family's id (the family check applies to
   main processes only, so a helper's inherited cross-family id still reaches a
   non-Grok owner).
4. Open `*.jsonl` files under `/.codex/sessions/`, `/.claude/projects/` or
   `/.grok/` are held by the process (`+pid` for a CLI main process, `−pid` for
   a helper). A session owns the holders of its own path and of every JSONL
   under its Grok directory (`events.jsonl` is what headless `grok -p` keeps
   open). **The one deliberate difference from `live.py`:** a `*.jsonl` whose
   link target lies under a configured read root (`SESSIONDOCK_CLAUDE_ROOT` /
   `CODEX_ROOT` / `GROK_ROOT`, prefix match on the configured spelling, the
   same one session rows use) counts as well. On the real machine the roots
   are exactly those homes, so this only widens synthetic trees.

"CLI main process" means argv0 (`claude`/`codex`/`grok`, `.exe` stripped,
`codex-*`/`claude-*` prefixes); a
runtime such as `node …/claude/cli.js --resume <id>` is not one (its `--resume`
id still counts, its inherited env id resolves to the nearest CLI ancestor by
`comm`, and it contributes no `started_at`). A Windows runtimes list
is only the candidate pre-filter on Windows and does not change this.

Per session list (`active_processes`): a process held by a Codex fork and its
ancestors belongs to the deepest fork in the list only; unrelated sessions
sharing a helper keep it; a session named by the Grok active file is live
without pids. `started_at[uid]` is the earliest start among the owned pids
that are CLI main processes (`btime + starttime / CLK_TCK`, unrounded).

## `tmux_uids`: whose console a pane is

`tmux_uids` are the live sessions that own a console: a `tmux*` ancestor
(12 levels) or a managed host's session root among
the ancestors (16 levels, `term_host.hosts` / `term.process_belongs_to`), a
managed instance that is `running` (always included), or a Claude session
that inherited a pane through `continued_in` (below).

- **CLI barrier** (`live.is_cli_process`, `ProcTree::is_cli_process`): both
  walks stop when an intermediate process — not the start of the walk, not
  the root or tmux server being looked for — is itself a CLI main process
  (`claude`/`codex`/`grok` argv0). A `grok -p` or `codex exec` spawned by a
  pane's Claude sits in that pane's process tree, but the console is the
  Claude's: the grandchild session is live (its own signals) and
  it is not in `tmux_uids`. A CLI's own tool
  shell (`bash tool.sh` under it) still walks through the CLI, because the
  CLI is the start's parent, not an intermediate.
- **Continued-in inheritance** (`server._pane_for_session` /
  `_continued_origin`, `api/runtime.rs` `inherits_pane`): a Claude session
  that no pane owns by process tree inherits the pane of the first listed
  row whose `continued_in` names it, recursively up to eight hops. The
  continued JSONL's process runs under the origin TUI's daemon child, so the
  origin's Claude is a barrier between it and the pane root; the list shows
  only the continued session, so the console follows it. The origin owns a
  pane when a verified host record declares its uid (a pane named
  `sessiondock-claude-<sid[:8]>`; Rust never matches names, the launcher's
  metadata is the declaration) or one of its own CLI processes descends from
  a host's session root. Only Claude sessions look for an origin; a spawned
  grandchild is not a continuation and never inherits. Without a configured
  host directory there are no panes to inherit.
- **Codex rollback branches** (`RuntimeSnapshot::fork_host`): `Esc Esc` in
  the Codex TUI forks a new thread whose `forked_from_id` names the current
  one, while the CLI process and the pane taken over for the parent stay
  where they are; the new rollout file is what the process now holds open,
  so `active_processes` folds the process onto the branch and the parent
  owns nothing. The host's verified binding keeps the parent's uid (the
  durable host identity never moves), so the branch has no binding of its
  own. The single host bound to a `codex_ancestor_sids` ancestor of the
  branch **and** whose session root hosts the branch's owned pids is the
  branch's host: takeover reuses it (`reused_process`), stop targets it,
  `ManagedResolver` delivers through it and the Codex approval probe
  captures it. A branch whose process moved on to a deeper branch, a root
  without ancestors, and a pair of candidate hosts resolve to nothing; the
  parent keeps resolving exactly and is `takeover_superseded` /
  `stop_superseded` once its pids folded away. The page follows the same
  graph from the list: a selected session that just became a hidden fork
  parent moves to the deepest branch (live sibling first, else newest),
  carrying the composer draft and the open console (`followSelectedFork`,
  `linkedTermSession` walking `forkAncestors` under the Rust backend), while
  a parent the user opened deliberately (fork chain, "显示父会话") stays put.
- **Codex thread switches** (`RuntimeSnapshot::codex_process_host`): a TUI
  can open an unrelated thread with no `forked_from_id`. An exact open rollout
  file identifies its CLI process and unique guarded host, ahead of declared
  UID and fork fallback. A second resume waiting for that rollout's lock must
  not displace the original TUI. Nested CLI processes do not inherit the outer
  CLI's host. `/api/term/list` exposes `current_uid` when unambiguous; immutable
  binding fields still guard every operation. Unrelated threads keep separate
  drafts and selection, even when they use the same TUI.

Rust has no tmux backend, so a pane the predecessor created in its own
tmux server is only recognised through the process-tree walk, never
inherited; that is the expected `tmux_uids` difference between the two
services on one machine.

## `/api/live` with the scan

`working_uids` lists already-live sessions with attributed command processes;
it does not add sessions to `uids` or authorize any process operation. A command
may retain `CODEX_THREAD_ID` / `CODEX_SESSION_ID`, `CLAUDE_CODE_SESSION_ID` or
`GROK_SESSION_ID` after detaching from its CLI. Known CLI ancestry takes
precedence over inherited identity. Permanent code-mode and recognized MCP
transport processes are excluded, while sleeping or I/O-bound commands count.
See [CLI state](cli-state.md) for the display precedence and exclusions.

```json
{
  "enabled": true, "known": true, "partial": false,
  "uids": ["claude:…", "grok:…"],
  "tmux_uids": ["claude:…"],
  "started_at": {"claude:…": 1789220318.44},
  "managed": null,
  "scan": {
    "enabled": true, "root": "/proc",
    "stats": {"processes": 3077, "matched": 181, "elapsed_ms": 64},
    "cache": {"hit": false, "age_ms": 0, "ttl_ms": 3000}
  }
}
```

`uids` follow the session list order (managed-only sessions appended);
`unavailable_reason` disappears; `managed` keeps the batch-22 shape when a host
directory is configured, with `external_detection: "proc_scan"`, and managed
`running` instances are merged into `uids`/`tmux_uids`/`started_at` (their own
start time is used when the scan has none). Local launch discovery initializes
`nest_parent` during sampling and in the background. A continuation of the same
conversation is not inferred as a newly launched child. A failed scan keeps the
managed answer, `partial: true` and `scan.status: "failed"`.

The frontend reads `uids`/`tmux_uids`/`started_at` and treats an unlisted
session as stopped. With `ui_events: true`, one `/api/events` SSE connection
notifies the page when live state, terminal metadata or list metadata changes.
A healthy stream replaces the periodic `/api/live`, `/api/term/list` and
`/api/sessions` browser polls. Initial connection and reconnect reconcile the
metadata; unsupported or disconnected streams retain polling as a fallback.

The event contains invalidation flags and changed session/agent cursors, never
conversation bodies. Cursor-only growth does not invalidate the full list.
Inactive views request only unread summaries, including previously cached
views; opening one loads its detail. Only the selected conversation subscribes
to the separate `/api/watch` body stream.

The current server observer shares cached metadata reads every two seconds
per debug view, irrespective of subscriber count, and stops when the last
subscriber leaves. This is browser push, not a claim that external CLI files
and processes already publish native lifecycle events. Changes are coalesced;
unchanged snapshots produce no application event, with a 20-second SSE
keepalive. Observation failure restores browser reconciliation until a fresh
baseline succeeds.

With `list_delta: true`, list reconciliation uses transport revisions as well:
the first response is complete, then only changed/deleted/inserted rows cross
the browser and hub-to-node connections. Unchanged terminal lists carry only
the current envelope and an empty delta. Missing revisions fall back to full
snapshots; history cursors, list ordering and offline rows retain their semantics.
The browser keeps its wire baseline separate from rows annotated by the UI.

A page that detects a newer server build pauses background HTTP reads, UI/history
SSE and terminal attachments until reload. Draft saving remains available.
An expired login (401 or a redirect to the proxy login page) pauses requests too
and displays the existing floating notice style, preserving editor contents.
These protections take effect after a pre-existing old page loads the new frontend;
deploying cannot replace JavaScript already running in another browser tab.

Settings → Features → Auto sleep defaults to one hour without interaction.
The browser saves the selected duration (5/15/30/60/120/240 minutes or disabled)
as `sleepMinutes` under its existing SessionDock preference namespace. Keyboard,
pointer, touch and wheel input reset the deadline; server output and returning
to a tab do not. Wall-clock checks also cover suspended computers and throttled
background timers. At the deadline a full-page modal uses the session-pause
shade and a **Resume** button, including above an already open settings dialog.
HTTP polling, history/UI SSE, audit beacons and terminal attachments pause;
the underlying CLI processes and editor drafts remain intact. Only Resume
dismisses sleep and reconciles the lists/history and terminal connection.
Recorded console replay reconnects at its saved position, and a terminal claim
that finishes after sleep starts cannot reopen a socket behind the overlay.
Resuming never clears a newer-build or expired-login pause.

## Response caches

The shared observer and fallback browser polls read `/api/live` and
`/api/term/list`; their answers are pure functions of source snapshots that have
freshness windows of their own. Since 2026-09-15 both routes keep the
assembled answer per debug-run view (`polls::PollCache`, at most eight views;
the predecessor's `_live_views` / `_panes`) and a hot request only checks
that the sources are the ones the entry was built from:

| route | key | expiry |
| --- | --- | --- |
| `/api/live` | the completed scan (`Arc` identity; 3 s TTL above), the shared managed observation (`Arc` identity; 2 s TTL, `None` without a host directory), the lifecycle generation, and the view's topology — the `SessionRow` fields of the visible rows in list order plus the uids the registry hides — so a session file that only grew keeps the entry | none of its own: the sources' TTLs bound it |
| `/api/term/list` | the lifecycle generation and the debug-run registry (`Arc` identity) | `TERM_LIST_TTL` = 2 s: host discovery and the receipt list carry no version |

The **lifecycle generation** (`LifecycleService::generation`, `0` without the
service) is a counter the coordinator advances after every mutating command
— create, kill/cancel, takeover, bind, native authorization, stop, discard —
from whichever caller (HTTP, the autobind task, the bug-report worker) and
whatever the outcome (a refused cancel may still have recorded its intent).
The shared managed observation is keyed on it as well
(`ManagedRuntime::observe_shared(force, generation, …)`; `invalidate()`
drops it outright), so an API mutation misses every display cache at once
while a change made behind the server's back — a host started by another
backend, a session file appended — shows up when its source refreshes,
within one poll interval. `?force=1` bypasses both caches (and the shared
observation) and re-populates them.

Only three fields of `/api/live` are per request and are filled in after the
lookup: `managed.cache` and `scan.cache` (`hit`, `age_ms`, `ttl_ms` of the
two source caches) and `scan.spawned_recorded` — a hit reports `0` (or
`null` without a state directory) because nothing was written by that call:
the entry's builder recorded this scan's spawners, and the 10 s tick
records anyway.

`/api/term/list` authorizes nothing, so it reads the shared observation
(the same one `/api/live` reads, `hosts` are identical) instead of a fresh
probe; claim, attach, unleased send, stop and process-evidence binding keep
`runtime::observe`, the fresh uncached probe. Its `sessions` and `pending`
rows are therefore at most 2 s old, the predecessor's `PANES_TTL`.

## Pending launches bound by process evidence

The same scan pairs a `Running` receipt of launch kind `new_pending` with its
native record: the host's child process is the root, and an indexed session
of the receipt's source whose owned CLI main process is or descends from it
(`ProcTree::hosted`, the CLI barrier included) is the candidate; exactly one
candidate is bound through the durable lifecycle bind with method
`process` ([lifecycle-http.md](lifecycle-http.md#automatic-binding-by-process-evidence)).
The scan stays read-only and cached exactly as above; the task only runs
while such receipts exist.

## `spawned_by`

Local process scans initialize the single `nest_parent` from observed launch
ancestry, both during `/api/live` assembly and on a ten-second background tick.
A newly created child's owned CLI process and its inherited identity/CLI ancestors
supply the evidence; OpenCode instead pairs a new top-level session with the
OpenCode processes running in its directory at birth, requiring unanimous
initiator evidence. A shared directory alone, a native OpenCode subagent, a
shared tmux server and a later resume do not supply that evidence. The first
recorded parent persists after exit. Explicit attach/detach wins across scans
and restart, as described in [metadata](metadata.md#sidebar-parent).
No `spawned_by` field is written. Verified SSH reports initialize the same
`nest_parent`, preserving explicit decisions;
native subagent relationships remain supplied by the CLI history.
Work that exits between scans without a recorded relation cannot be recovered
from timestamps or a shared working directory alone.

### SSH boundary

Linux direct SSH discovery uses the shared [process-links](process-links.md)
node reports and Hub coordinator. OpenSSH exposes `SSH_CONNECTION` on the remote side as
client address/port and server address/port ([ssh(1)](https://man.openbsd.org/ssh.1)).
That tuple is connection evidence, not a parent session identity. A direct,
unshared connection could be correlated with the launching node's SSH socket and
its process ancestry while both sides remain alive. `ControlMaster` shares one
connection across multiple sessions, so the tuple alone cannot identify a channel's
launcher; jump hosts and NAT also require additional evidence.

A per-launch parent identity could extend coverage to these ambiguous cases.
The current collector skips them. Ordinary session variables are not automatically
forwarded: `SendEnv` needs the server's `AcceptEnv`, and neither changes the
meaning of an existing CLI's own identity variables
([ssh_config(5)](https://man.openbsd.org/ssh_config.5)). Direct connections need no
forwarded marker. Node-scoped resolution, native creation-order checks and durable
process incarnations protect the recorded relationships. Complete short-lived
process coverage and non-Linux adapters remain in [TODO](../TODO.md).

The launcher refuses `CODEX_THREAD_ID`, `CODEX_SESSION_ID` and `CLAUDE_PID`
in profile environments in addition to the session ids, so a web-created
session is never recorded as the child of whatever session started the
service (the launcher `env_clear`s).

## Validation

- `python3 tests/live_http_suite.py --binary … [--ptyhost …]` — scan over a
  synthetic tree: uids/tmux_uids/started_at, envelope, cache hit and force
  miss, no inferred sidebar parents, persistence across restart with
  an empty tree, `SESSIONDOCK_GROK_ACTIVE`, a `grok -p` under a tmux pane's
  claude (live, not in `tmux_uids`); with ptyhost
  built, a real pane resuming the origin whose synthetic subtree runs the
  continued session and a `grok -p`: `tmux_uids` = origin + continued only.
- `python3 tests/metadata_suite.py` (seeded `nest_parent` row),
  `python3 tests/check_config_suite.py` (the three variables) and
  `python3 tests/spawned_by_suite.py` (six-session tree with a `node` CLI,
  a companion Codex, a headless Grok holding `events.jsonl` under the Grok
  root, an orphan helper and a tmux pane whose claude spawned a second
  headless Grok — `tmux_uids` names the claude only; expectations verified
  against `live.py`).
- Real roots, read-only: the debug binary with the real roots on a loopback
  port versus the deployed
  service's `/api/live`; the uid sets and the reasons for every difference are
  recorded in the batch ledger.
