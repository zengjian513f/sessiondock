# One-time host native binding

`launch_bind_v1` records one trusted operator's association between a launched
host and a native session. It does not inspect native records, discover a CLI,
infer a SID, launch a process, allocate a browser lease or enable reliable send.
It is a separate outer operation, not an optional field on legacy operations and
not an inner operation accepted by either existing guard envelope.

## Wire contract

All successful Info replies advertise integer `capabilities.launch_bind: 1` at
the response root and include root `native_binding`. A capable, unbound host
returns JSON null; missing capability means an older implementation, not a
verified unbound host. Existing capability fields remain unchanged.

```json
{
  "op": "launch_bind_v1",
  "token": "private-local-transport-token",
  "expected_instance_id": "synthetic-instance-0001",
  "expected_source": "codex",
  "expected_launch_id": "synthetic-launch-0001",
  "native": {"sid": "complete-native-id", "uid": "codex:complete-catalog-uid"}
}
```

Existing socket/token authentication runs first. Unix permits an omitted token;
Windows requires the existing token. An included token must be a string. All
three expected identity values must exactly match the receiving Session's
immutable spawn metadata. Instance/launch IDs use the existing 16–128 ASCII
identifier rule; source is exactly `claude`, `codex`, or `grok`. SID and UID each
require 1–256 ASCII letters/digits or `_ . : -`; UID must begin with the source
and a colon followed by a nonempty suffix. Both native fields are required.
Unrecognized fields at either request level, mixed envelopes, malformed values,
and missing identity are rejected before mutation. The host's three-source wire
support does not assert that the application's NativeScope supports all three.

Successful first binding and exact replay have the same reply shape:

```json
{
  "ok": true,
  "launch_guard": {
    "version": 1,
    "instance_id": "synthetic-instance-0001",
    "source": "codex",
    "launch_id": "synthetic-launch-0001"
  },
  "native_binding": {
    "version": 1,
    "instance_id": "synthetic-instance-0001",
    "source": "codex",
    "launch_id": "synthetic-launch-0001",
    "sid": "complete-native-id",
    "uid": "codex:complete-catalog-uid",
    "method": "operator"
  }
}
```

The binding reply does not include `instance_guard`. `method` is fixed by the
host; it is not a user-supplied claim of native-header or process verification.
The same complete binding object appears at the root of subsequent raw,
launch-guarded and native-guarded Info replies, alongside `info`, `exited`, and
`capabilities`. It is never nested in `info` or `info.meta`.

Rejections contain no submitted identity, credential, path, or native content:

```json
{"ok":false,"error":"native binding rejected","code":"native_binding_rejected"}
{"ok":false,"error":"native binding conflict","code":"native_binding_conflict"}
```

Conflict is returned only for a valid, correctly launch-guarded request whose
native tuple differs from the already published tuple. Malformed requests or
wrong launch identity remain generic rejections, even after binding.

## One-time publication and lifetime

Only a valid pending launch whose original metadata has neither a `sid` nor a
`uid` field may be bound. A null, malformed, partial or already declared native
field also prevents binding; this API does not adopt or upgrade legacy metadata.

The state is one in-memory optional, complete binding per Session. First binding
holds the same child mutex as process wait/stop across `try_wait` and publication.
Only `Ok(None)` and a host not known exited permit first publication. A reaped
child or probe error fails closed. Publication is the linearization point;
concurrent different proposals have exactly one winner. No update, clear, or
partial field addition exists. An exact replay returns the published result
without another child poll, including during exit; that ACK is not liveness.

The nested lock order is child then binding. A binding request waiting for the
child does not hold the binding mutex or prevent Info snapshots. No socket,
filesystem, PTY, screen/backlog operation, or thread join runs under the binding
mutex. Snapshots and acknowledgements copy a complete object before sending it.

Spawn metadata and the on-disk host record are unchanged. In particular, binding
does not call `write_info()`, and later attach/resize/rename record refreshes do
not persist or overwrite the binding. The existing directory record remains an
endpoint-discovery hint; clients obtain binding evidence from an authenticated,
identity-guarded reply of the actual host.

The host survives a Web restart, so a newly constructed client can observe the
binding through launch-guarded Info using the receipt's full instance/source/
launch tuple. This is not persistence across host termination. A new host begins
unbound; a stale file or old Web cache cannot restore a binding into it. An
implementation must not reuse process instance nonces.

A lost ACK can follow successful publication. The caller should first obtain
fresh launch-guarded Info: the same tuple confirms the association, a different
tuple is conflict, and an unavailable host leaves the result unknown. Explicit
repetition of the exact binding tuple is safe; this does not permit automatic
retry of send, paste, keys or other non-idempotent host operations.

## Compatibility and trust boundaries

`launch_guard_v1` continues checking only the immutable launch identity. Existing
pending attachments remain the same byte streams and keep their input authority;
binding neither disconnects nor upgrades them.

`guarded_v1` retains its legacy metadata checks for hosts without a new binding.
For a bound pending launch it checks the one-time native tuple instead: both
`expected_sid` and `expected_uid` must be present and exact, alongside source and
instance. Its existing `instance_guard` acknowledgement stays unchanged. It does
not accept a nested binding operation. Legacy raw operations remain available,
but putting native fields on raw Info/send/attach does not bind anything. Older
hosts reject the new unknown outer operation without executing an inner effect.

The trusted application/operator must establish the relationship. Native records
alone prove no relationship to a running host. The service should resolve the
supported main-session `NativeScope` (no agent selector) and use its actual
`session_id` and full UID, not a filename or display SID. A native record may
legitimately have a display SID different from its true native ID. Explicit
operator confirmation remains necessary until a separately reviewed adapter can
provide stronger process-association evidence. The host cannot distinguish an
operator from other callers possessing equivalent local transport access.

Browser ownership and lifecycle policy remain above this protocol. A pending
lease must not silently become a native lease; explicit release/new claim and
fresh native evidence are separate steps. Cancellation must retire any native
lease derived from the same launch/instance as well as its pending lease, without
revoking an unrelated replacement solely by name. No such lease transition is
performed by this host operation, and binding itself grants no reliable-send or
native-file mutation capability.

## Validation

`cargo test -p ptyhost --test host_native_binding --locked` uses only built development ptyhost processes,
fixed free POSIX shells, explicit private `--dir`/cwd, and a cleared environment.
It checks simultaneous socket binds, complete Info snapshots, lost ACK recovery
on a new connection, preservation of an already attached pending stream,
native-guarded input, root-only projection, rename/record invariance, conflicts,
wrong identity and old-entry-point rejection. Cleanup addresses only the private
socket and the held test Child, not a PID loaded from a record. No native CLI,
paid model, production session or real native history is used.

Windows compilation is checked separately; Linux shell tests do not establish
Windows or macOS runtime behavior.
