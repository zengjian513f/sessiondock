# Host launch identity envelope

`launch_guard_v1` identifies one explicitly launched host process before a native
session SID or UID is available. It is independent of the existing `guarded_v1`
native identity envelope. It does not discover a native session, establish native
ownership, create a launcher, or bind/update a running session's metadata.

## Capability and request

The ordinary Info response advertises protocol support at its root:

```json
{
  "ok": true,
  "capabilities": {"instance_guard": 1, "launch_guard": 1},
  "info": {"meta": {}},
  "exited": false
}
```

`capabilities.launch_guard` is the integer version `1`, not a boolean and not a
field inside `info`. It advertises host implementation support; it does not claim
that the receiving session has usable launch metadata. A client must separately
validate the metadata and every guarded response.

The envelope is:

```json
{
  "op": "launch_guard_v1",
  "token": "private-local-transport-token",
  "expected_instance_id": "synthetic-instance-0001",
  "expected_source": "codex",
  "expected_launch_id": "synthetic-launch-0001",
  "request": {"op": "attach", "cols": 80, "rows": 24, "replay": true}
}
```

Existing transport authentication runs first. Unix retains its private socket
authentication, where `token` may be omitted; Windows requires the existing host
token. An included token must be a string. No credential is put into a guard ACK
or rejection message.

The receiving `Session` checks its immutable metadata against all three required
expected fields before dispatching the inner request:

| Expected field | Immutable metadata | Required value |
| --- | --- | --- |
| `expected_instance_id` | `instance_id` | 16–128 ASCII bytes from `A-Z a-z 0-9 _ . : -` |
| `expected_source` | `source` | Exactly `claude`, `codex`, or `grok` |
| `expected_launch_id` | `launch_id` | 16–128 ASCII bytes from `A-Z a-z 0-9 _ . : -` |

All fields must match exactly. SID and UID are neither required nor inferred.
Extra fields in the envelope, including `expected_sid` and `expected_uid`, are
rejected. Unknown or nested inner envelopes are also rejected: launch cannot wrap
native guard, native guard cannot wrap launch, and launch cannot wrap launch.

The inner request uses the same validated operation set as `guarded_v1`: `info`,
`cursor`, `send`, `paste`, `keys`, `resize`, `attach`, `capture`, `rename`, and
`kill`, with the existing field and size validation. Cancelling an owned pending
launch uses guarded `kill`; there is no new `cancel` wire operation.

## Response and side-effect boundary

Every successful launch-guarded control response, including the initial attach
JSON acknowledgement, adds this exact independent object:

```json
{
  "launch_guard": {
    "version": 1,
    "instance_id": "synthetic-instance-0001",
    "source": "codex",
    "launch_id": "synthetic-launch-0001"
  }
}
```

It does not add `instance_guard`. Native `guarded_v1` keeps its original
`instance_guard` ACK and does not add `launch_guard`. Legacy operations add neither.
Failed operations do not carry a success guard ACK. Invalid launch envelopes
receive the generic response:

```json
{"ok": false, "error": "launch guard rejected", "code": "launch_guard_rejected"}
```

Validation, comparison, and dispatch refer to the same immutable `Session`, not
a filesystem record reopened after an earlier probe. Rejection precedes attach
resize, output queue registration/replay, PTY input, key/paste translation, rename,
or kill. Rename changes the endpoint name without changing the guarded identity.
Attach uses the existing ordered output boundary: the complete launch ACK is
admitted before replay and live byte frames.

An old host that does not implement `launch_guard_v1` rejects the unknown outer
operation instead of executing its inner operation. Clients must not put launch
fields on a legacy `send`/`attach`, fall back to a name-only operation, substitute
`guarded_v1`, or automatically retry an ambiguous mutation after submission.
Legacy operations themselves remain available and unchanged for compatibility;
this feature does not make every host caller use identity guards.

## Deliberately separate from native association

Launch identity is not proof of a native SID or UID. A pending launch with no
native metadata remains unable to satisfy `guarded_v1`'s existing native checks.
The host performs no one-time native binding, identity promotion, memory mutation,
or launcher lifecycle state transition. A later native-association mechanism
needs a separate design and authorization boundary.

Identity values are supplied at spawn. Format validation cannot prove randomness
or detect a launcher reusing them; production launchers must create fresh process
instance IDs and manage launch IDs explicitly. PID liveness, socket names, and an
earlier Info response do not replace the receiving process's envelope check.

## Validation

`cargo test -p ptyhost --locked` covers valid pending operations and separate ACKs,
all required identity fields and their format boundaries, generic errors, extra
fields, malformed effects, mixed/nested guard refusal, unchanged legacy behavior,
and no native-guard promotion.

`crates/ptyhost/tests/host_launch_guard.rs` runs fixed free `/bin/sh` fixtures in
private temporary `--dir` directories. It exercises real Info/attach/replay/send,
rename with unchanged metadata, guarded cancellation, rejected side effects,
missing metadata, and same-name replacement with stale instance or launch IDs.
Cleanup addresses only its private socket and held `Child` process handle; it
does not kill PIDs recovered from records. These tests exercise the current host;
the old-host unknown-operation behavior is a protocol compatibility property,
not a claim that historical host binaries were executed by this test.

Linux host tests and Windows `x86_64-pc-windows-msvc` compilation are checked
separately. Cross-compilation is not Windows or macOS runtime validation.
