# Trusted native scope selection

`SessionStore::native_scope(uid, agent)` returns
`Result<NativeScope, SessionError>`, where `NativeScope` has public `source`,
`uid`, `session_id` and optional `agent_id` fields. It is a session-domain type
and has no dependency on the delivery implementation. Like other synchronous
session reads, callers must use the bounded blocking reader executor.

| Selection | `uid` | `session_id` | `agent_id` |
| --- | --- | --- | --- |
| Claude main | Inventory owner UID | Owner's explicit native JSONL `sessionId` | `None` |
| Claude child | Same owner UID | Same owner `sessionId`, checked against the child's records | Exact owned agent ID |
| Codex main | Inventory owner UID | Native `session_meta.payload.id` | `None` |
| Codex child | Inventory owner UID | Selected child's native `session_meta.payload.id` | Exact owned child ID |
| Grok | — | — | Scope resolution returns 501 |

Resolving a child proves which native view was selected and is passed through
to delivery rather than rejected by an application-level agent policy.
Successful Claude scope selection is
identity evidence, not permission to write to a terminal or run a CLI.

## Identity evidence and snapshot consistency

The native parser collects scope provenance once from the already parsed,
complete JSONL records. It never reopens a file for scope resolution. Claude
requires an explicit nonempty `sessionId`; Codex requires an explicit
`session_meta.payload.id`. Neither source falls back to a filename, title,
display SID, Codex `session_id` alias or cursor hash. IDs over 256 UTF-8 bytes,
non-string IDs, whitespace-only IDs and IDs containing control characters are
rejected for scope selection.

An unfinished JSONL tail is not identity evidence. Missing or invalid identity
returns 501. Conflicting explicit identity declarations return 409, as does a
Claude child declaring a different session ID from its owner. Existing history
validation may reject malformed/unsupported records earlier. These extra
identity checks are stored as an internal result and do not change ordinary
list/message display compatibility or turn a readable history into an error
merely because its delivery identity is unproven.

Scope construction runs inside the same inventory-backed `Graph::select` /
`history::resolve` used by message views. Agent IDs are exact owned inventory
entries; prefixes, `agent-` filename guesses, another owner's child, a direct
child UID and request-supplied paths do not bypass ownership. Unknown selections
return 404, ambiguous selections 409, and existing graph/size errors remain
explicit. The selected immutable view retains both its display metadata and
the independent native-scope result.

`native_scope` reuses `SessionStore::snapshot`: relevant parent/child dependency
files are restamped and changed files reparsed together before publication;
inventory discovery retains the existing shared 500 ms refresh policy.
Unchanged parsed files and views are reused. Owner and child identity never
come from two separate public reads that could observe different revisions.
A scope already returned is evidence for that snapshot, not an ongoing lease;
any future send workflow must validate the current target, operation and
native cursor before acting.

## Why display metadata cannot define a scope

Claude agent parsing replaces `parsed.meta.sid` with the agent ID, and logical
view rendering sets `meta.sid` to the agent-menu ID again. An agent's native
records can simultaneously declare the parent's `sessionId`. The cursor's
`native_identity` is also a hash of the physical source/UID/agent, not a native
session ID. A scope built from any of those display/cursor fields can miss
existing receipts because the delivery engine compares every scope field.

Filtering only by owner UID and ignoring
the agent query, and omitting the outbox driver for child views, does
not establish a child-delivery scope contract. Rust resolves the full native
identity explicitly and never substitutes the main outbox for a child scope.

## Validation

Run `cargo test -p sessiondock --lib sessions::native_scope_tests --locked`.
The synthetic tests cover owner/native/display identity separation, exact
agent selection, missing/invalid/conflicting IDs, mismatched Claude child
session IDs, incomplete tails, immutable old snapshots, cached reads, Codex
native IDs versus display aliases, and explicit unsupported Grok scope.
Ordinary history/display and byte cursors remain unchanged in these scenarios.
Fixtures live only in private temporary directories. No native CLI homes,
production sessions, predecessor state, network or paid agent CLI is accessed.
