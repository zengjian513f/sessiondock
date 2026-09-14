# Controlled process observations

This optional capability only observes configured host records. It does not
discover CLI homes, control processes, or grant terminal access. Ordinary
inventory reads do not clean records; lifecycle recovery may retire an exact
unchanged Linux record after the operating system proves its host process dead.
It requires `SESSIONDOCK_PTYHOST_DIR`; unset disables host observations. External CLI processes use the
Python-shaped native scan of [liveness.md](liveness.md), which `/api/live`
merges with the observations described here.

## Evidence, not name inference

The Python backend names existing terminal sessions using
`sessiondock-{source}-{sid[:8]}`. Its `term_host.new_session` invocation does not
currently supply ptyhost's `--meta` argument. The Python live subsystem's
process arguments, environment, file descriptors, working directories and time
heuristics live in the separate scan ([liveness.md](liveness.md)); none of them
is imported into this catalog.

Eight-character name prefixes, process labels, working directories, timestamps,
and PID existence are not native-session identity evidence. Empty metadata is
expected for legacy-created hosts: they remain observable raw terminals but
cannot be associated with a native UID by this feature. No upgrade rewrites their
metadata or guesses the missing identity.

The existing host already accepts immutable JSON metadata. A future launcher
can supply only the reviewed association fields, for example:

```json
{
  "source": "codex",
  "sid": "complete-native-session-id",
  "uid": "codex:0123456789abcdef",
  "instance_id": "unique-random-instance-nonce"
}
```

`source` must be exactly `claude`, `codex`, or `grok`; at least one of full `sid`
or full `uid` is required. IDs are case-sensitive, at most 256 ASCII identifier
characters (`A-Z a-z 0-9 _ - . :`), without whitespace or path separators. A UID
must start with the declared source and a colon. There is no trimming, prefix
matching, SID synthesis, or source guessing. `instance_id`, when present, is
16–128 identifier characters; a future launcher must generate a fresh,
cryptographically random value for every process instance. Length/shape
validation does not prove another launcher generated its nonce correctly.

Unknown metadata keys are discarded. Wrong types, null reviewed values, invalid
source/IDs, or malformed instance IDs yield `invalid_metadata`, not partial
acceptance of the remaining fields. They do not make a valid host transport
record disappear. Tokens, endpoints, full argv, and arbitrary metadata never
appear in observation DTOs or error text.

## Exact native catalog boundary

`NativeCatalog::from_rows(&[Value])` accepts the SessionStore's frozen public
session-list rows. The runtime performs no native filesystem I/O of its own.
It indexes full UID and `(source, full SID)` identities, retaining duplicates.

- If both SID and UID are supplied, each must identify exactly one row and both
  must identify the same row.
- Missing, ambiguous, conflicting, unsupported, or subagent identities stay
  unknown, with a typed reason. Agents are not silently attributed to a root
  session. Distinct fork sessions retain their own native identities.
- Multiple verified hosts declaring the same native UID are all marked
  `duplicate_host`, including a running/exited pair. The runtime does not select
  a preferred host by PID, name, creation time, or attachment state.
- An unreachable host's stale record does not become association or liveness
  evidence. The record is left untouched.

## Liveness and instance boundaries

`HostClient::probe(name)` reads the private host record, connects to its bounded
local endpoint, submits `Info`, then rereads the record. Name, host/child PIDs,
creation time, endpoint, private credential, command label, cwd, and reviewed
immutable metadata must agree before/reply/after. A mismatch returns
`identity_changed`. Mutable attachment state and terminal dimensions may change.

TCP is restricted to IPv4 loopback and uses the record's token; Unix uses the
socket derived inside the explicit trusted directory. Private record tuples and
tokens are compared only internally. They are never exported as reusable
credentials or claimed to be a persistent process-instance identity.

Only a successful, consistent host reply sets a host's `known:true`:
`exited:false` means the host reports its child running; `exited:true` is confirmed
child exit at observation time. Missing/refused endpoints, auth failures, invalid
protocol, replacement races, and timeout mean `unknown`, not exited. Linux tests
do not establish Windows or macOS execution coverage.

## Process identity and the three run states (batch 22)

On Linux the runtime additionally captures a `ProcessIdentity {pid, start_time}`
for the host's child and for the host itself, reading only
`/proc/<pid>/stat` field 22 (start ticks since boot) of PIDs named by an already
verified host record, plus `/proc/stat btime` and `/proc/self/auxv AT_CLKTCK`
for the boot clock. Nothing enumerates `/proc`, sends signals or reads a process
owned by another user (`not_owned`). The first observation of an instance
(keyed by name, host PID, child PID, creation time and instance nonce) must show
the child started no later than five seconds after the record was written
(`started_after_record` otherwise); every later observation rereads both stat
lines and requires exact equality (`mismatch`). Identities are compared in ticks,
never in converted wall-clock seconds.

On Windows (WP-W) the same evidence comes from the kernel's process object:
the PID is opened with `PROCESS_QUERY_LIMITED_INFORMATION` only, a process
that has already exited but whose PID is still reserved by an open handle
(this service's own retained `Child` included) reads as `not_visible`, the
owner check compares the process token's user SID with this service's
(`not_owned` when different, and also when the token cannot be opened — never
a pass), and the start time is the creation `FILETIME` of `GetProcessTimes`
in 100 ns units after the Unix epoch, so `latest_start_for` and `started_at`
keep their meaning with a clock of `boot_time 0` and `10_000_000` ticks per
second (`process_identity: "windows_process_times"`). Nothing enumerates the
process table. Other platforms return `unsupported_platform`: the absence of
a process table is never an exit.

Per native UID the snapshot folds the evidence into exactly one of:

- `running`: the host answers Info with `exited:false` **and** child/host
  identity verified, for a uniquely matched UID (`evidence:"host_info"`).
- `exited`: the host reports `exited:true` (`host_exit`); a lifecycle binding
  for the same instance is `Exited` and `Confirmed` (`exit_receipt`); or an
  identity that this Web process verified earlier has disappeared from `/proc`
  after the host stopped answering or its record vanished (`identity_gone`).
  The last form is Linux/Windows-only, limited to instances verified in this
  process lifetime, and reverts to `unknown` after a Web restart; without it a real
  ptyhost's exit (record cleaned within ~100 ms) would be nearly unobservable.
- `unknown` with a typed reason: `no_instance`, `host_unreachable` (with
  `probe_error`), `record_missing`, `duplicate_host`, `identity_unverifiable`
  (with the failure), `platform_unsupported`.

Precedence is current instance evidence, then remembered identity, then exit
receipts; evidence from a different instance never overrides the current one.
`sessions` lists only UIDs with evidence; every unlisted UID is `unknown`
(`unlisted:{state:"unknown",reason:"no_instance"}`) — an empty list never means
"all sessions stopped".

Association can be `matched` while `identity_unverified:true`: a full native SID
can identify a history without identifying a particular process incarnation.
Only an explicit launcher instance nonce clears this flag. Even then this
snapshot is read-only and can become stale immediately. A future session control
API must bind and freshly revalidate native UID + host name + instance before
claim/attach and serialize this with ownership changes. This feature deliberately
does not enable session-console routing or reuse a bare host name as authority.

## HTTP and resource bounds

`GET /api/live` keeps the legacy envelope. On a platform without native
process discovery and without a configured host directory it is
`enabled:false`, `known:false`,
`partial:true`, empty `uids`/`tmux_uids`/`started_at`, `managed:null`. With a
host directory it answers `enabled:true`, `known:true`, `partial:true` (with an
`unavailable_reason` explaining that only explicit host instances are
observed), `uids` = UIDs currently `running`, `started_at[uid]` from the
verified child start time, and `managed` carrying `process_identity`
(`linux_proc`, `windows_process_times` or `unsupported`), `observed_at`, the host rows (each with
`process:{status: verified|unverifiable{reason}|reaped|unchecked, child, host}`
and `started_at`), `sessions[uid] = {state, evidence|reason, host,
instance_id, pid, started_at}`, `unlisted`, and `cache:{hit, age_ms, ttl_ms}`.
All additions are additive; legacy only reads `uids`/`tmux_uids`/`started_at`.
`external_detection` is `not_implemented` where native discovery is unsupported: unmanaged external
CLIs are then outside this inventory, absence is never a negative liveness
assertion or a reason to take over/stop a session, and the `live` capability
stays `false` because the legacy `live:true` semantics treat the list as the
complete set and would mark unlisted sessions as stopped. On Linux the native
scan supplies that complete set:
`partial:false`, `external_detection:"proc_scan"`, `live:true`, and the
managed `running` instances are merged into `uids`/`tmux_uids`/`started_at`
([liveness.md](liveness.md#apilive-with-the-scan)).

Concurrent `/api/live` requests share one cached snapshot (TTL two seconds,
single-flight refresh, `?force=1` bypasses the TTL but still serializes);
`/api/term/list` and claim keep the fresh, uncached observation. The identity
memory holds at most 1024 instances, evicting gone/oldest entries first;
re-checking stale memory runs under the same deadline and concurrency limits.

The endpoint has a separate admission limit of two requests, uses the bounded
blocking session reader to freeze the native catalog, responds `no-store`, and
cancels host I/O on shutdown. Admission exhaustion is an explicit 503
`runtime_busy`; inventory errors/budgets give 503 `runtime_unavailable`. Individual
host errors remain typed unknown rows. No automatic retry or write occurs.

Runtime defaults are 256 hosts, eight parallel probes, two seconds per probe,
and a five-second whole snapshot deadline including discovery. Exceeding the
host count fails the snapshot rather than silently omitting records. Remaining
hosts after the shared deadline receive timeout/unknown rows. The HTTP service
uses a dedicated client with a 64 KiB metadata/control-line limit and a 512-entry
directory limit rather than the larger terminal-replay client defaults.

Validation uses artificial native catalog rows, copied synthetic JSONL fixtures,
and loopback fake peers. Tests cover exact matches and ambiguity, duplicates,
subagent rejection, source/UID conflict, empty legacy metadata, exited versus
unreachable, identity replacement, redaction, deadlines, HTTP admission, and
shutdown. The earlier temporary free-shell smoke test validates the
independent transport; this catalog does not execute a paid or real native CLI.

An opt-in Unix smoke test also runs one fixed free shell with synthetic metadata,
checks the real host's Info response through two fresh Web app instances, and
exits/reaps that shell. After building the local ptyhost, run it explicitly with
`SESSIONDOCK_TEST_PTYHOST_BINARY=$PWD/target/debug/ptyhost cargo test -p sessiondock
--test runtime --test runtime_live --locked -- --ignored`; `runtime_live` walks a
real instance from `running` (PID equals the record) through `quit` to
`exited`/`identity_gone`. `python3 tests/live_browser.py` checks in Chromium that
a managed instance's console works, turns into the grey exit explanation after
`quit`, and that sessions without an instance keep "运行状态未知" (the active
filter refuses to hide them), on desktop and 390 px widths; legacy never
requests `/api/live` on its own under the Rust capabilities.
