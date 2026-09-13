# Instance-bound terminal protocol

Browser ownership and host process identity solve different problems. A lease
authorizes one page/connection to write; an instance guard prevents that lease
from accidentally reaching a new process behind the same terminal name. Neither
PID existence nor a native SID alone proves a process incarnation.

## Reproduced boundary

The original local protocol uses one JSON request per connection. After a valid
Info probe, a same-name record/endpoint can change before the next attach/send.
`tests/observation.rs` reproduces a previous successful probe followed by a raw
name-only attach receiving `NEW_INSTANCE`. This is intentional evidence that the
unbound compatibility API is not sufficient for session-associated controls.

Adding an optional `expected_instance_id` to an existing `send`/`attach` is not
sufficient: an old host ignores unknown JSON keys and can execute the operation
before a client notices a missing guard acknowledgement. Even an earlier Info
capability check cannot close the replacement race between connections. The
second fake-peer regression demonstrates that downgrade boundary.

## Backward-compatible envelope

New hosts advertise this explicit capability in the Info response root:

```json
{"ok": true, "capabilities": {"instance_guard": 1}, "info": {}, "exited": false}
```

Instance-bound clients use a new outer operation that old hosts reject without
executing an inner command:

```json
{
  "op": "guarded_v1",
  "token": "private-local-transport-token",
  "expected_instance_id": "unique-launcher-instance-nonce",
  "expected_source": "codex",
  "expected_sid": "complete-native-session-id",
  "request": {"op": "attach", "cols": 80, "rows": 24, "replay": true}
}
```

The token remains private and is only for existing local transport auth. The
expected native keys come from the verified immutable host metadata: source and
at least one of SID/UID; if both were declared, both are sent and must match. The
client also stores the full resolved native source/SID/UID in its `BoundTarget`,
even when the host declares only one native identifier. Native catalog uniqueness
must be established by the calling service, not guessed in the host client.

The host authenticates the outer request first, validates the complete guard and
inner request, and compares against the receiving session's immutable metadata
before dispatching any operation. In particular, guard failure must precede
attach resize, replay registration, PTY writes, paste/key translation, rename,
or kill. Nested guarded envelopes and malformed guard fields are rejected.
Comparison and dispatch act on the same session instance; a filesystem record
check in the Web service is not a substitute.

Every successful guarded response includes exactly:

```json
{"instance_guard": {"version": 1, "instance_id": "unique-launcher-instance-nonce"}}
```

For attach this is part of the initial JSON acknowledgement before byte frames.
The client requires version 1 and the exact expected nonce before exposing replay
or a writable attachment. Coalesced acknowledgement/replay bytes are preserved.
Missing, malformed, wrong-version, or mismatched guard acknowledgement is an
error. A response after submission may be ambiguous about whether an operation
already took effect: there is no automatic retry, resend, or unguarded fallback.

Existing clients using the original operation names keep the original protocol.
New bound clients refuse observations without an explicit capability or strong
launcher instance ID. If a formerly capable endpoint is replaced by a legacy
host, `guarded_v1` is rejected as an unknown operation before side effects.
Instance nonces must be freshly generated for each launcher-created process;
format checking cannot detect a launcher incorrectly reusing a nonce.

## Rust API and integration boundary

`BoundTarget::from_observation(observation, source, full_sid, full_uid)` validates
capability, nonce, native metadata, and non-exited observation. It is cloneable
but not Debug/Serialize and holds no host credential. The caller must supply an
exact, supported, unique main-session identity from its frozen catalog.

`HostClient::request_bound` and `attach_bound` first check the current private
record against this target, then send only the guarded envelope. The host-side
guard closes the remaining record-read/connect/dispatch race. Once attach is
acknowledged, later filesystem endpoint replacement cannot redirect its existing
connection to another process.

The browser service now pins the complete `BoundTarget` in the lease through
`claim_bound` and validates UID/instance again in `prepare_bound`. Target storage
is reclaimed with lease expiration/release/replacement, without an unbounded side
map. Raw claim/prepare cannot downgrade an existing bound lease. The per-terminal
IO gate is retained during the fresh claim probe, guarded attach, and writes;
force replacement publishes ownership under that same gate. Already transmitted
bytes cannot be recalled; cleanup of an old connection cannot release a newer
lease. The read-only `/api/live` catalog alone grants none of these permissions.

Client fake-peer tests cover stale record rejection before connecting, endpoint
replacement with unchanged metadata, legacy-host refusal without downgrade,
strict guard acknowledgement, coalesced replay/raw byte framing, and ambiguous
timeout handling. Host-side enforcement and the Web bridge must be validated
separately; fake peers are not evidence that an older host binary enforces guards.

## Output completion is separate from identity

An exact instance guard identifies a stream but does not guarantee complete PTY
output. Typed exit frames preserve optional `output_complete` and a whitelisted
reason (`pty_drain_timeout`, `pty_read_error`, or unknown). Missing completeness
means legacy/unknown, not verified success. A reason without explicit incomplete
status is malformed. Bare stream EOF is not a process exit marker.

The Web bridge queues prior bytes before its close event, and the legacy page
preserves that rendered tail. Only an explicit host exit retires automatic
reconnection for the pinned instance; network disconnects do not establish that
the process died. See [host output](host-output.md) and
[terminal ownership](terminal-ownership.md) for budgets and browser acceptance.
