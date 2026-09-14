# ptyhost-client

An asynchronous client for the **local** ptyhost wire protocol. The Web server's
private terminal bridge and read-only host catalog use this library.
It does not launch processes, select production sessions, implement browser
WebSockets, infer CLI acceptance, or provide a general terminal manager.

## Explicit access only

```rust,no_run
use ptyhost_client::{ControlOp, HostClient, Limits};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let client = HostClient::new(".runtime/ptyhost", Limits::default())?;
let records = client.discover().await?;
if let Some(record) = records.first() {
    let reply = client.request(&record.name, ControlOp::Info).await?;
    // Match the typed reply in the terminal application service.
    let _ = reply;
}
# Ok(()) }
```

The constructor does not discover environment, home, or default paths. Relative
paths use its cwd. Tests must use a private development directory, never production state.

`discover` reads bounded regular JSON files and returns sorted public summaries.
It does **not** connect, inspect PIDs, delete stale files, or prove the host is
alive. Missing directories are empty; malformed/name-mismatched/symlink metadata
is skipped. Other discovery errors and budgets are explicit. A synthetic PID is
not interpreted as a dead process, including on macOS where `/proc` is absent.
After a failed exact-instance probe, the lifecycle caller may explicitly invoke
`retire_if_local_process_dead`: on Linux it removes only the same unchanged record
whose boot ID differs, whose legacy creation time predates this boot, or whose
host PID is gone/zombie. Missing exact random launch names also mean exited.

Tokens are private deserialization-only data, never part of Debug/Serialize
public DTOs. `argv`, `sock`, `port`, and untyped `meta` are also omitted from
public summaries. `probe` separately exposes only the reviewed association fields
described below, never arbitrary maps. Error messages do not copy peer-provided text or
malformed JSON because those can echo credentials. This intentionally trades
some diagnostic detail for predictable redaction.

`probe(name)` adds a read-only `HostObservation`: safe summary, `exited`, optional
launcher `instance_id`, and `AssociationState::{Missing,Invalid,Declared}`. The
declared association accepts only exact source (`claude`/`codex`/`grok`) and full
SID and/or source-prefixed UID. IDs are bounded ASCII identifiers; instance nonces
are 16–128 characters. It verifies the private identity tuple before the Info
request, in the reply, and after a record reread. Changes to identity, endpoint,
credential, or reviewed immutable metadata return `IdentityChanged`; mutable
size/attachment changes are allowed. Unknown metadata keys are discarded.

`declared(name)` reads only the reviewed metadata a record declares (safe
summary, association, instance nonce) without connecting or probing liveness,
so an unreachable host's `unknown` row can still be named; callers must never
treat it as a verified association.

Observation is not control authority. PID and creation time cannot prove a
persistent instance; absent launcher nonce remains explicitly unknown. Neither
a record's existence nor a failed Info probe proves a running/exited child.
Ordinary discovery performs no `/proc` probes, fallback name-prefix associations,
or cleanup. The explicit dead-record reconciliation above is separate from
runtime association and never treats a live but unreachable process as exited.

`BoundTarget::from_observation` additionally requires explicit protocol capability
and a launcher nonce, then pins the caller's exact full native source/SID/UID.
`request_bound`/`attach_bound` use `guarded_v1`, never a plain operation with an
ignorable extra field. Old hosts reject this new outer operation before acting.
Bound replies require an exact versioned instance acknowledgement; missing/wrong
ack is ambiguous, never retried or downgraded. Regular `request`/`attach` retain
their original unbound transport semantics. See `docs/terminal-identity.md` in
the workspace for the protocol and separate browser ownership requirements.

`LaunchTarget::from_observation(observation, source, full_launch_id,
expected_instance_id)` provides a separate guarded identity for a launched host,
including a pending host with no native SID/UID. Both nonces must be explicit
16–128 character ASCII identifiers and exactly match the reviewed observation;
callers obtain them from their trusted creation receipt, not a host-name guess.
The peer must advertise root-level `capabilities.launch_guard: 1` (integer).
Missing, boolean, string and unknown versions do not enable the capability.

`status_launch(name, source, launch_id, instance_id)` can also observe an exited
instance, but still requires exact guarded Info acknowledgement and matching
records within one operation deadline. It returns only an observation, never a
writable target; `LaunchTarget::from_observation` continues to reject exited hosts.

`HostObservation.native_binding` distinguishes Unsupported, Unbound, Invalid and
Bound. It is decoded from the root of the actual Info reply, never disk metadata.
`status_launch` returns the latest guarded reply's binding, not a prior probe's
snapshot. `bind_launch(&LaunchTarget, sid, uid)` is a separate `launch_bind_v1`
operation requiring explicit trusted operator scope; it is not a raw ControlOp.
Exact same-tuple retries are idempotent but never automatic. Lost ACKs are unknown
until guarded status confirms the complete binding. The returned `NativeBinding`
has reviewed getters only; method `operator` is an assertion, not native proof.

`BoundTarget::from_observation` accepts such a binding only for the exact source,
native SID/UID and instance. Its `origin_launch_id()` remains pinned so browser
lease retirement and lifecycle cancellation cannot be bypassed by switching to
native control. A caller must independently validate the real NativeScope and
durable lifecycle receipt. Missing/invalid bindings never downgrade a bound target.
Existing immutable-native metadata targets keep their original behavior.

`HostObservation.launch` is `LaunchState::{Missing,Invalid,Declared}`;
`Declared` contains only reviewed `LaunchIdentity { source, launch_id }`, with
the instance nonce remaining in the existing `instance_id` field. Missing,
malformed or short launch/instance values never form a target. Launch parsing
is independent of native `AssociationState`: valid launch metadata cannot
manufacture a native association, and invalid launch metadata does not erase
an otherwise valid native declaration. An ordinary pending object with source
but no SID/UID retains the existing invalid-native-association classification.
The before/reply/after Info identity comparison includes reviewed launch data;
unreviewed metadata, credentials, argv and peer error text remain excluded.

`request_launch` and `attach_launch` use only this envelope:

```json
{
  "op": "launch_guard_v1",
  "token": "<private local host credential>",
  "expected_instance_id": "<complete instance nonce>",
  "expected_source": "codex",
  "expected_launch_id": "<complete launch nonce>",
  "request": { "op": "kill", "force": false }
}
```

Success requires `launch_guard` with exactly
`{ "version": 1, "instance_id": "…", "source": "codex", "launch_id": "…" }`.
Every field and the complete shape are checked. The current record must still
match the target before connecting; the receiving host must check its immutable
identity before effects. Guarded Info also verifies its returned record, and
rename replies must name the exact requested destination. Missing/wrong ACK,
EOF and timeout can follow an already applied operation: no automatic retry,
raw-name downgrade or fallback to the native guard occurs. Attach exposes
neither replay nor a writable handle before the complete launch ACK matches.

Launch control does not start a process, persist a creation receipt, bind a
native session, authorize a browser page or confirm delivery. It cannot be
converted into `BoundTarget` when native SID/UID evidence is missing. Those
application workflows remain separate. Existing native-bound and raw transport
methods keep their established semantics.

## Transport and compatibility

- TCP always uses IPv4 `127.0.0.1`, a nonzero metadata port, and a nonempty token.
  A metadata-supplied hostname/IP is never used. This branch is testable on Linux
  and is the Windows transport. If a port exists it takes precedence over sock,
  matching the Python Web client's selection.
- On Unix, the socket is derived as `<explicit-dir>/<validated-name>.sock` and
  must be an actual socket, not a symlink. Metadata `sock` must have that basename;
  its directory prefix is not used to redirect the connection. This supports
  records written with relative host directories without trusting arbitrary
  socket paths. The directory itself is an explicit trusted input; this is not
  a sandbox against another process already able to replace its contents.
- Control requests use JSON lines, one request per connection. Replies become
  typed info/capture/cursor/paste/rename/ack variants. Capture/cursor preserve
  `lag`, `dropped`, and `resets`; absent fields mean unknown, not zero. Composer
  or approval logic must decide whether that screen is trustworthy.
- Attach preserves all bytes after its JSON acknowledgment, including coalesced
  replay frames. Frames are `kind:u8 + length:u32 big-endian + payload`.
  Kind 1 is raw data; outbound kind 2 is resize JSON; inbound kind 3 is exit JSON.
  Unknown host frame kinds are rejected. No UTF-8 transformation is performed.
- `TerminalSize` enforces nonzero dimensions. Browser-specific hidden-view size
  filtering belongs in the Web layer; this client does not silently resize.
- Current metadata requires the existing host's PID/shape fields. There is no
  negotiated protocol version yet; extension fields are ignored, not forwarded.

Default budgets are 4 MiB per control line/metadata file, 64 MiB per attach frame,
and 5 seconds per control/open operation. Discovery enumerates every host
record and attached reads have no partial-frame deadline, matching Python. The
replay allowance includes the existing host's 32 MiB model backlog plus history.

An attached host may be silent indefinitely, including after a partial frame.
Callers may opt into a fixed partial-frame deadline retained across cancellation.
Readers can safely be used in `tokio::select!`. Failed/truncated frames terminate
the reader. A cancelled or failed writer cannot be reused, because part of its
frame may already have reached the host. Drop both halves and reconnect if needed.
`shutdown` only closes this client's input; it does not send `kill`.

**Never blindly retry input after an error.** Even control timeout/EOF can happen
after the PTY accepted the write. Reliable delivery needs the provider-specific
durable ledger and native transcript confirmation planned in M5. Dropping an
attach connection is separate from terminating its host/CLI process.

## Tests and remaining work

The default `cargo test -p ptyhost-client` uses temporary directories, fake
TCP/Unix listeners, and synthetic metadata only. No ptyhost binary, shell, paid
CLI, home-directory session discovery, or production service is started.

Tests cover privacy/read-only discovery, path/name/size constraints, token
authentication, typed control replies, split/coalesced JSON and frames, exact
bytes, resize, exit/EOF, deadlines, cancelled reads, cancelled/failed writes,
frame budgets, and symlinks.
Launch tests additionally exercise pending probe/control/attach, metadata
independence/redaction, exact capabilities/nonces/ACK, changed records, stale
or legacy peer rejection, coalesced replay, and ambiguous writes without retry.
Linux test success is not Windows/macOS runtime validation.

An ignored Unix interoperability test explicitly starts a built ptyhost with a
fixed free `/bin/sh` in a private temporary directory, with both working
directories explicit and its inherited environment cleared (only a fixed PATH
and TERM are supplied). Build the host, then set
`SESSIONDOCK_TEST_PTYHOST_BINARY` to its exact absolute executable path and run:

```sh
cargo build -p ptyhost --locked
SESSIONDOCK_TEST_PTYHOST_BINARY=/absolute/build/path/ptyhost \
  cargo test -p ptyhost-client --test launch_host --locked -- --ignored
```

It does not guess a binary from cwd or discover native sessions. It checks typed
pending observation/control/attach, exact byte input and complete exit, host-side
rejection of a forged synthetic launch record without kill/resize effects, and
same-name replacement rejecting the old target. This is not native CLI startup
or binding validation.

Application-owned concerns remain process startup/platform lifetimes, leases,
WebSocket bridging/backpressure, terminal/session UID association, provider
composer parsers and delivery state machines; they are not provided by this
client library. Host slow-consumer and handshake/replay/live ordering require
their own focused validation beyond the launch interoperability scenario.
