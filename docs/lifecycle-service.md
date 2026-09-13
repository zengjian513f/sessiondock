# Bounded lifecycle coordinator

`lifecycle::service::LifecycleService` coordinates the existing private receipt
store, explicit [launcher](lifecycle-launcher.md), guarded ptyhost client and the
application's shared `Arc<TerminalService>`. It does not infer native SID/UID,
authorize a model CLI by source name, or establish reliable-send acknowledgement.
This is a local, explicitly configured create/status/cancel chain, not completion
of the lifecycle migration.

## Public library contract

`open(directory, launcher_config, terminal, limits, cancellation_token)` only opens
an initialized ledger. It never initializes, selects defaults, resets
corruption or discovers CLI homes. Configuration supplies server-owned CLI definitions; see the store
and launcher contracts. The passed terminal service must be the same instance
used for browser claims. The caller must enforce configuration/root separation
and HTTP authorization. The service never guesses native scope.

- `create(request_id, LaunchSpec)` durably prepares and starts a new intent,
  performs one spawn, then returns its private receipt. Exact duplicates query
  the existing intent and cannot spawn again; conflicting specs fail.
- `get(record_id)` and `list(offset, limit)` refresh launched records using exact
  instance evidence. Lists include all retained receipts without a fixed record quota.
- `target(record_id)` performs a fresh observation and returns an immutable
  `Arc<LaunchTarget>` only for a verified Running, non-cancelled instance. This
  target still requires guarded revalidation when terminal ownership is claimed.
- `cancel(record_id, expected_instance_id)` durably records cancellation before
  local lease retirement or one guarded kill attempt. Wrong instances fail.
- `discard(record_id, expected_instance_id)` (WP-E) refreshes the receipt and,
  only when it is Exited/Failed or durably cancelled, persists the display
  tombstone that hides it from `term/list.pending`. It never kills, deletes
  or re-authorizes anything; a live receipt is `NotReady`.
- `shutdown().await` closes admission, explicitly rejects queued work, lets
  already-started blocking work finish, and waits for the store lock to release.

`bind(VerifiedNativeBinding)` and `authorize_native(&BoundTarget)` add explicit
operator-confirmed (or, since WP-E, process-evidence) association and fresh
authorization for native targets derived from a launch; the
`lifecycle::autobind` task builds the process-evidence authority from the
managed runtime observation and the `/proc` scan and calls the same `bind`. See [the binding contract](lifecycle-binding.md) for strict scope,
schema-3 persistence, same-value explicit retries and read-only recovery. They use
the same coordinator and budgets; no ordinary query can initiate binding.

Records/specs contain private cwd data. They are internal values, not an HTTP
schema. Callers must project only approved fields and must not serialize launcher
configuration, raw host metadata, tokens, argv, environment or filesystem errors.
Service errors contain typed, fixed classifications, not raw paths or command
output. HTTP handlers must distinguish unavailable/uncertain from empty or live.

## Admission, blocking work and cancellation ownership

One coordinator serializes admitted commands. A bounded channel and semaphore
allow eight total queued/working/unread-response requests by default; tests may
choose one through eight. Full admission immediately returns Busy. The permit
stays with the request and its oneshot response, including after the HTTP future
is dropped. Cancelling a response cannot cancel a persistence operation, lose a
spawn result, release its capacity early or release the store lock prematurely.

All store reads, JSON encoding, recovery, fsync, launcher validation/spawn and
final store destruction run off the Tokio reactor. Only one such task is active
per coordinator. Opening work waits for its permit. The spawn/join operation is never timed out or aborted: after
a successful OS spawn there is no safe timeout that can discard its Child.

The library permit ends when a public method returns its private Record or target.
It is **not** a budget for subsequently serialized HTTP bodies. The transport must
hold a separate bounded response permit until its body finishes or is dropped.
The main HTTP integration uses this independent response budget; the service does
not add a second public receipt schema or build unbounded encoded response buffers.

Default readiness budget is five seconds, cancellation three seconds, each host
operation one second, and polling 50 ms. Limits have explicit maxima of 30, 10,
2 and 1 seconds respectively. A list shares one readiness observation deadline
across all rows, not a separate five-second wait per host. Once it expires,
unverified rows become Uncertain. Required bounded ledger fsyncs are still awaited:
a network deadline is not a promise that disk I/O can be safely interrupted.

## Create, observe and recover

The durable sequence is Prepared → Starting → spawn once → guarded Info → Running.
The exact `StartAuthority` is consumed only after matching instance/source/launch
evidence; a display name alone is never identity. Spawn failure produces typed
Failed. Exit before readiness is Failed; readiness timeout or shutdown after spawn
is Uncertain, not a false queued/running result. No native queue or strong CLI
acceptance is inferred from readiness, terminal output or a timestamp.

Every successful spawn immediately transfers its exact `std::process::Child` to
a process-wide reaper before leaving blocking work. The reaper has one thread and
a growing collection of owned children. It uses `try_wait`, retains wait-error
handles as unknown, and removes jobs only on known child exit. It never kills children. No service/store is retained by the
reaper. Reopening in the same process can subscribe to a still-owned Child only
by matching host root and complete record/name/source/launch/instance identity;
completed jobs are removed, not retained as an unbounded tombstone map. Thus
response drop, service Drop and graceful Web shutdown do not kill a
host or retain its ledger lock. Process crash cannot preserve an in-memory Child;
reopening reconciles through guarded host status, never PID/name-only inference.

Starting recovers durably as Uncertain and never gains another spawn authority.
Prepared after a crash remains non-reauthorizing and may be cancelled; any new
request is an explicit new intent, not an automatic retry. Historical Running
is not presented as live without a fresh exact status check. Guarded explicit
host exit or an exact owned Child exit can prove Exited. Unreachable endpoints,
wrong instances, missing guards and socket errors prove only Uncertain. An
Uncertain non-cancelled record may recover to Running with fresh exact evidence.

## Durable cancellation and schema compatibility

Schema 2 introduced a required `cancel_requested` flag and CancelRequested state. Opening
a strictly valid schema-1 ledger migrates it with `cancel_requested:false` and a
durable commit, preserving every request/spec/identity. Mixed old/new fields,
unknown schemas and malformed histories fail closed; no record is discarded.
Migration/recovery write failures return errors and release the failed open's lock.
Current schema 3 additionally migrates old records with `binding:null` and recovers
historical binding confirmation as Uncertain until fresh guarded Info matches.

For a launched instance, `request_cancel` first durably sets CancelRequested and
returns a handle-bound, non-Clone authority for that exact revision. The service
then retires the full launch identity in the shared terminal service, revoking
only that instance's leases, before attempting a guarded kill. Retirement capacity
failure cannot trigger a kill. A cached exact target may retire an offline former
owner; guarded host I/O still rejects a replacement instance at the same name.

A kill ACK is not Exited. Only subsequent exact exit evidence finishes Exited;
timeout, lost ACK, replacement or unavailable observation finishes Uncertain with
the cancel flag retained. Duplicate cancellation may retry idempotent **local
retirement** after a transient gate/admission failure and refresh observations,
but cannot issue a second kill, even when the first result was lost. On restart CancelRequested
becomes Uncertain with the flag still set; it never reissues kill authority and
never regains a claimable target. A still-live cancelled host remains visibly
uncertain and requires explicit operator reconciliation; automatically retrying
against a new instance would be unsafe. Prepared cancellation does not spawn or
kill. Already-exited exact status can finish cancellation without sending kill.

Observations carry exact record ID, launch ID, instance ID and revision. The store
rejects stale observations and old-handle cancellation tokens. The durable store
reloads current ledger data and reports actual persistence failures. Nothing here
claims exactly-once external execution or support for arbitrary filesystems.

## Validation

Synthetic service tests exercise queued admission and stalled blocking work,
dropped responses, shutdown, spawn rejection, idempotency,
guarded readiness/restart, fresh status, replacement and wrong-instance rejection,
durable cancellation, ACK-without-exit uncertainty, lost responses, crash recovery,
no duplicate kill, corruption handling, and missing-ledger rejection.
Store tests additionally cover strict schema migration, exact-revision evidence,
old authorities and all injected cancellation persistence-failure boundaries.

The ignored `explicit_free_shell_creation_survives_response_drop_and_shutdown_then_cancels_exact_host`
test requires both `SESSIONDOCK_TEST_PTYHOST_BINARY` and
`AGENTHUB_TEST_FREE_SHELL_BINARY` as explicit absolute binary paths. It runs only an
temporary free shell in private directories, verifies response-drop
ownership, live-host survival across service shutdown/reopen, and exact guarded
cancellation. Never substitute a model CLI or a production host directory.
