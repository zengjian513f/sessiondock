# Controlled creation: integration contract

This contract has a private Linux runtime: explicit launcher,
bounded coordinator, create/status/cancel HTTP and launch-bound pending WS.
`terminal_create` remains false by default; it is enabled only after opening
the configured lifecycle ledger and CLI profiles. Batch ten adds explicit
operator native binding below; no real model CLI launch contract, automatic
process association or reliable-send queue is claimed.

The implemented prerequisites are documented separately in
[host launch identity](host-launch-identity.md) and
[lifecycle receipt storage](lifecycle-store.md). The client exports a distinct
`LaunchTarget`; the store exports one-use Prepared/Start authorities. The
coordinator connects them as described in [service](lifecycle-service.md),
[launcher](lifecycle-launcher.md) and [HTTP/legacy acceptance](lifecycle-http.md).

## Identities and authority

| Identity | Meaning | Not interchangeable with |
| --- | --- | --- |
| request ID | Idempotency key for one normalized creation specification | Host name, native SID |
| launch ID | One durable attempt to launch an explicitly configured adapter | Native SID or display UID |
| instance ID | Non-reused nonce of one receiving ptyhost process | PID, creation time, endpoint name |
| native SID/UID | Verified session identity from the selected inventory snapshot | cwd, short filename, launch ID |

The host validates launch identity against its own immutable metadata before
performing an inner operation. A record-file check alone does not close the
record-read/connect/dispatch race. `LaunchTarget` is independent from
`BoundTarget`: a valid pending launch must not become a native-bound target just
because the browser supplies a UID. A name is an endpoint locator only.

## Creation coordinator

1. Resolve a server-configured CLI and an absolute working
   directory. Browser requests cannot provide executable paths, shell snippets,
   arbitrary argv/environment, or credentials. Adapter versions/parameters and
   capability advertisements must reflect verified contracts, not old
   invocation patterns alone.
   Legacy `adapter_id` input does not select a command; source selection follows
   the fixed CLI table.
2. Persist the canonical specification and generated launch/instance identities.
   The same request and specification return the prior receipt; a conflicting
   specification is rejected. Do not evict idempotency records silently when the
   bounded ledger fills.
3. Persist `Starting` before consuming the one-use launch authority. Keep the
   coordinator alive independently of the HTTP response; client cancellation
   must not lose a launched process or permit a duplicate spawn.
4. Launch an explicit ptyhost binary with an explicit private `--dir`, fixed
   generated name and immutable launch metadata. Do not inherit a default native
   home or production environment in tests. Do not pipe undrained
   stdout/stderr or kill the host when its Web request is dropped.
5. Observe the exact host, require the advertised launch guard, and perform a
   guarded Info exchange before recording readiness. A created metadata file or
   a successful `spawn()` alone is not a usable pending console.
6. Return pending status pinned to the receipt and instance. An ambiguous spawn,
   lost acknowledgement, or `Starting` after restart is reconciled, not retried.
   Reconciliation must not kill a same-name replacement or choose a candidate by
   directory/time. A process that started but never became observable remains an
   explicit uncertain result.

The coordinator must retain process handles for its own children while running,
reap them without blocking the async reactor, and distinguish host exit from
mere transport loss. Web shutdown/restart detaches from established hosts; it
does not terminate their CLI sessions. Linux process groups alone do not prove
survival of a service manager's cgroup stop policy; on Windows the host breaks
away from the service's job object when that job allows it and otherwise stays
inside it ([lifecycle-launcher.md](lifecycle-launcher.md#validation)); macOS
behavior requires its own runtime validation before advertising parity.

## Pending terminal and cancellation

Pending sessions have their own typed lease target: receipt + launch + instance.
They do not enter the native session catalog under a fabricated UID. The legacy
pending view is minimally adapted with HTTP status/list and WS routing together.
Claim/force/attach capture the same target and never
fall back to a name-only connection.

Cancellation is a serialized lifecycle operation. Revoke the exact instance's
input lease before guarded kill. An ACK means the request was received, not that
the process has already exited. Keep an uncertain receipt until observed exit;
do not repeatedly kill whichever process later occupies the endpoint. Rename
must share the instance operation gate across old/new names, revoke stale leases
and confirm the same identity at the new endpoint before publishing success.

## Native binding remains a separate protocol

Some adapters may eventually supply a verified preallocated native SID. Others
need a startup receipt, narrowly scoped process/file evidence, or an explicit
operator-confirmed association. NativeScope must independently validate the
actual record and owner. Ambiguity, missing records, unsupported provider scope
or an inaccessible process are not permission to choose the newest file.

Immutable metadata cannot be upgraded by editing a host JSON file. Batch ten
implements a separate write-once host state: launch-guarded, same-tuple idempotent,
conflicts rejected, root Info observable after Web restart without altering meta.
See [host protocol](host-native-binding.md) and [durable service](lifecycle-binding.md).
The HTTP endpoint requires explicit operator confirmation plus a verified main
NativeScope. That is an operator association assertion, not automatic CLI proof.
Pending leases remain launch-bound; switching to native requires explicit release
and fresh authorization. Cancelling the launch also retires its derived native
leases, and its durable flag blocks new native claims after Web restart.

## Temporary-environment acceptance

Use an explicit fake executable or fixed free shell, never a paid model CLI.
Verify the whole creation/status/real browser input/cancel chain, duplicate
requests, conflicting specs, every persist/spawn/ACK crash window, delayed
readiness, same-name replacement, and Web restart while preserving the host.
Test native appearance and multiple competing records with synthetic files;
assert their original bytes remain unchanged. A fake executable validates the
coordinator and protocol, not a real CLI's arguments, billing or native receipts.
