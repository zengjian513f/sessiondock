# External CLI liveness: the `/proc` scan and `spawned_by`

The Python `live.py` process scan sits next to the managed
host observations ([processes.md](processes.md)). On platforms with
native process discovery, `/api/live` exposes the complete set Python's
frontend expects (`capabilities.live: true`).

## Scope

- `SESSIONDOCK_PROC_ROOT=<dir>` (default `/proc`) is the process table to read;
  tests point it at a synthetic tree.
- Grok's active-sessions file defaults to Python's
  `~/.grok/active_sessions.json`; `SESSIONDOCK_GROK_ACTIVE=<file>` provides a
  test override. Entries name sessions live without a pid, stale ones included.
- On a target without native process discovery, `/api/live` reports
  `scan: {status: "unsupported_platform"}`, `capabilities.live` stays false and
  managed observations remain available. External checks return no PID evidence
  and therefore never signal a process. Python uses psutil on Windows; Rust may
  add the equivalent provider independently of the HTTP and lifecycle contract.

The scan is read-only: it lists the table, reads `cmdline` of every process,
and `environ`, `cwd` and `fd/*` links of the processes whose command line
mentions `claude`, `codex` or `grok`; `stat` lines are read lazily for ancestry
walks. It never signals, writes, follows a link outside the tree, or elevates
privileges (another user's `environ`/`fd` are unreadable and simply skipped;
`cmdline` is world-readable and Python reads it too).

Results are cached for 3 s counted from the moment a scan completes (a slow
scan must not expire its own result), refreshes are single-flight (waiters
reuse the one in progress) and `?force=1` bypasses the TTL while still
serializing — Python `snapshot`. On this machine (256 cores, ~3000 processes,
~180 keyword matches) one debug-build scan takes 60–70 ms; Python's takes
~125 ms.

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
   process never takes another family's id (Python applies the family check to
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
`codex-*`/`claude-*` prefixes), exactly Python's `_is_cli`/`_cli_family`; a
runtime such as `node …/claude/cli.js --resume <id>` is not one (its `--resume`
id still counts, its inherited env id resolves to the nearest CLI ancestor by
`comm`, and it contributes no `started_at`). Python's `_WINDOWS_RUNTIMES` list
is only the psutil candidate pre-filter on Windows and does not change this.

Per session list (`active_processes`): a process held by a Codex fork and its
ancestors belongs to the deepest fork in the list only; unrelated sessions
sharing a helper keep it; a session named by the Grok active file is live
without pids. `started_at[uid]` is the earliest start among the owned pids
that are CLI main processes (`btime + starttime / CLK_TCK`, unrounded).

## `tmux_uids`: whose console a pane is (Python 16cc89c)

`tmux_uids` are the live sessions that own a console: a `tmux*` ancestor
(12 levels, Python `term_tmux.hosts`) or a managed host's session root among
the ancestors (16 levels, `term_host.hosts` / `term.process_belongs_to`), a
managed instance that is `running` (always included), or a Claude session
that inherited a pane through `continued_in` (below).

- **CLI barrier** (`live.is_cli_process`, `ProcTree::is_cli_process`): both
  walks stop when an intermediate process — not the start of the walk, not
  the root or tmux server being looked for — is itself a CLI main process
  (`claude`/`codex`/`grok` argv0). A `grok -p` or `codex exec` spawned by a
  pane's Claude sits in that pane's process tree, but the console is the
  Claude's: the grandchild session is live (its own signals) and
  `spawned_by` the Claude, and it is not in `tmux_uids`. A CLI's own tool
  shell (`bash tool.sh` under it) still walks through the CLI, because the
  CLI is the start's parent, not an intermediate.
- **Continued-in inheritance** (`server._pane_for_session` /
  `_continued_origin`, `api/runtime.rs` `inherits_pane`): a Claude session
  that no pane owns by process tree inherits the pane of the first listed
  row whose `continued_in` names it, recursively up to eight hops. The
  continued JSONL's process runs under the origin TUI's daemon child, so the
  origin's Claude is a barrier between it and the pane root; the list shows
  only the continued session, so the console follows it. The origin owns a
  pane when a verified host record declares its uid (Python's pane named
  `sessiondock-claude-<sid[:8]>`; Rust never matches names, the launcher's
  metadata is the declaration) or one of its own CLI processes descends from
  a host's session root. Only Claude sessions look for an origin; a spawned
  grandchild is not a continuation and never inherits. Without a configured
  host directory there are no panes to inherit.

Rust has no tmux backend, so a pane the Python service created in its own
tmux server is only recognised through the process-tree walk, never
inherited; that is the expected `tmux_uids` difference between the two
services on one machine.

## `/api/live` with the scan

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
    "cache": {"hit": false, "age_ms": 0, "ttl_ms": 3000},
    "spawned_recorded": 0
  }
}
```

`uids` follow the session list order (managed-only sessions appended);
`unavailable_reason` disappears; `managed` keeps the batch-22 shape when a host
directory is configured, with `external_detection: "proc_scan"`, and managed
`running` instances are merged into `uids`/`tmux_uids`/`started_at` (their own
start time is used when the scan has none). `spawned_recorded` is the number
of spawners written by this call (`null` without a state directory, an
`{error}` object when the metadata store refused). A failed scan keeps the
managed answer, `partial: true` and `scan.status: "failed"`.

Legacy reads only `uids`/`tmux_uids`/`started_at`; with `live: true` it polls
`/api/live` on its own and treats an unlisted session as stopped — which is now
correct, as with Python.

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

`runtime/spawn.rs` ports `live.spawn_parents`: for every live session's CLI
main process the ancestor chain (16 levels including the process itself) is
walked. A level names a spawner when it is another listed session's owned
main process, when its environment carries another listed session's
`CLAUDE_CODE_SESSION_ID` / `CODEX_THREAD_ID` / `CODEX_SESSION_ID` /
`GROK_SESSION_ID` (`SPAWN_ENV`, lowercase sid lookup by `(source, sid)`), or
when `CLAUDE_PID` names another session's owned pid. A `tmux*` server ends
the walk (its environment belongs to nobody). Among several candidates the one
with the latest `created` wins (a child is born after its parent). Candidates
are memoised per scan snapshot.

The relation is visible only while both processes exist, so it is persisted
immediately: every `/api/live` records it, and a background task ticks every
10 s because headless fan-outs live and die while
no page is open. Both run only when the scan and `SESSIONDOCK_STATE_DIR` are
configured. The metadata row key is `spawned_by: {source, sid}`, written once
and never rewritten ([metadata.md](metadata.md#spawned_by)); rows of
`/api/sessions` carry it verbatim. `tests/meta_import.py` converts Python's
key as-is (the spawner may no longer exist).

The launcher refuses `CODEX_THREAD_ID`, `CODEX_SESSION_ID` and `CLAUDE_PID`
in profile environments in addition to the session ids, so a web-created
session is never recorded as the child of whatever session started the
service (Python strips `SPAWN_ENV_KEYS` from its own environment at startup;
the Rust launcher `env_clear`s).

## Validation

- `cargo test -p sessiondock --lib runtime::procscan runtime::spawn
  api::runtime metadata::` — every `tests/test_live.py` and
  `test_session_meta.py` case over a synthetic tree (`FakeProc`): bare claude
  window, orphan helper, command line beats environment, cross-family, Codex
  fork folding, headless Grok `events.jsonl`, Grok active file, tmux/host
  ancestry, caps, TTL/single flight/force, the seven-session spawn fixture,
  depth 16, memo and write-once; plus one real `/proc` scan that prints its
  timing. `test_host.py` PaneOwnershipTests (the CLI barrier on both walks)
  and `test_server.py` PaneLinkingTests (origin keeps its pane, the continued
  session inherits it, the spawned grandchild has no console, the eight-hop
  bound, first listed origin wins) are ported verbatim.
- `python3 tests/live_http_suite.py --binary … [--ptyhost …]` — scan over a
  synthetic tree: uids/tmux_uids/started_at, envelope, cache hit and force
  miss, `spawned_by` on the rows and on disk, persistence across restart with
  an empty tree, `SESSIONDOCK_GROK_ACTIVE`, a `grok -p` under a tmux pane's
  claude (live, `spawned_by` the claude, not in `tmux_uids`); with ptyhost
  built, a real pane resuming the origin whose synthetic subtree runs the
  continued session and a `grok -p`: `tmux_uids` = origin + continued only.
- `python3 tests/metadata_suite.py` (seeded `spawned_by` row),
  `python3 tests/check_config_suite.py` (the three variables) and
  `python3 tests/spawned_by_suite.py` (six-session tree with a `node` CLI,
  a companion Codex, a headless Grok holding `events.jsonl` under the Grok
  root, an orphan helper and a tmux pane whose claude spawned a second
  headless Grok — `tmux_uids` names the claude only; expectations verified
  against `live.py`).
- Real roots, read-only: the debug binary with the real roots on a loopback
  port versus the deployed Python
  service's `/api/live`; the uid sets and the reasons for every difference are
  recorded in the batch ledger.
