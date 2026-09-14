# Explicit operator native binding

Native binding associates an existing lifecycle launch instance with an explicitly
selected, validated native main-session scope. It is an **operator assertion**,
not proof that a CLI in the host created that native history, accepted a prompt,
or acknowledged reliable delivery. There is no filename/name-based discovery,
native history write, or model CLI invocation.

A second asserting party uses the same durable path: the server's
own process evidence (`BindingMethod::Process`, `lifecycle::autobind`,
described in [lifecycle-http.md](lifecycle-http.md#automatic-binding-by-process-evidence)).
`VerifiedNativeBinding::from_process_evidence(scope, receipt, note)` replaces
the operator confirmation with an evidence note naming the CLI pids under the host's child and the held native
record; everything below — intent before host call, one bind per request and
confirmation only by matching guarded Info — is
unchanged. `BindingRecord` carries `method`, `evidence` and `bound_at` (Unix
seconds of the first confirmation); an operator retry of the same spec keeps
the first method/evidence.

## Confirmation and private data

The transport must obtain a current main `NativeScope` from the configured native
catalog and independently require explicit operator confirmation. It passes that
scope and the private receipt to
`VerifiedNativeBinding::from_scope(&scope, &record, operator_confirmed)`.
This constructor rejects missing confirmation, subagents, source disagreement,
non-Running/cancelled receipts and malformed identifiers. Grok is bindable
(its native scope is `summary.json` `info.id`).
It does not reread native files: the caller owns current catalog validation and
HTTP authorization. `NativeScope` is trusted internal input, not an HTTP body.

The resulting authority has private fields and implements neither serde, Clone
nor Debug. It pins record ID, launch ID, instance ID and the full binding spec.
`BindingSpec` itself is serializable private **data**, not authority; deserializing
a spec cannot call the service binding API. The coordinator rechecks the current
receipt and guarded host state after admission, so an older UI confirmation cannot
bypass later cancellation or instance replacement.

The private persisted model is `Record::binding(): Option<&BindingRecord>` with
`BindingRecord::{spec(), state()}` and `BindingSpec::{source(), sid(), uid()}`.
State is Intent, Confirmed or Uncertain. UID must have its exact source prefix and
a nonempty suffix. Identity strings are supplied whole from validated native
scope, never truncated display IDs. Retained records and ledger bytes have no fixed quota.
No public HTTP schema is introduced by this library; full specs/receipts remain
private and errors never embed SID, UID, paths or host credentials.

## Durable host operation

`LifecycleService::bind(VerifiedNativeBinding)` uses the existing single bounded
coordinator, admission permit and blocking persistence boundary:

```text
fresh exact Running + uncancelled receipt and host
  → durably commit immutable BindingSpec as Intent
  → at most one host bind call for this explicit request
  → fresh same-instance guarded Info
  → Confirmed only when its complete binding matches; otherwise Uncertain
```

The host capability must be present before a new intent is accepted. A conflicting
existing receipt or host binding fails without replacing the spec. New Intent is
committed before returning a handle/revision-bound store authority. Known write
failure freezes before any host call; post-rename failure stays uncertain. The
exact intent cannot be reinterpreted to a different source/SID/UID.

A successful bind ACK alone does not confirm the receipt. A lost ACK followed by
matching guarded Info may be reconciled; a lost ACK plus missing status stays
Uncertain. If an ACK claims success but Info is Unbound, it remains Uncertain.
Explicit same-value requests may retry the idempotent host operation after another
durable Intent revision. An already-confirmed matching request needs no host write.
If the host is already bound to the exact chosen scope but this ledger has no
intent, an explicit operator request can persist the intent and confirm with Info;
ordinary reads cannot adopt it automatically.

HTTP response cancellation does not cancel admitted work or release its permit
early. Shutdown does not start a host bind after it has been observed; a committed
intent with no conclusive result remains Uncertain. A bind already sent may have
taken effect even when its response was lost. This is not exactly-once execution.

## Recovery and fresh observation

Schema 3 adds the required nullable `binding` field. Strict valid schema-1 and
schema-2 ledgers migrate durably by adding `binding:null`, preserving all prior
requests, identities and cancellation flags. Schema 1 also receives the previously
defined `cancel_requested:false`. Schema 5 adds `method:"operator"`,
`evidence:null` and `bound_at:null` to every migrated binding object. Old
envelopes with missing required current fields or mixed schemas fail closed;
ordinary extra fields follow tolerant dictionary reads.

On open, historical Intent becomes Uncertain in the same durable recovery commit
used for Starting/CancelRequested. A Confirmed binding survives restart because
it records an association already established by guarded Info; it is not live
control authority. No new binding authority is issued. Schema-5 process bindings
that an older Web recovery downgraded are restored only when both their process
evidence and original `bound_at` confirmation remain. An observed exit of the exact
instance keeps a Confirmed binding on the Exited receipt: that pair is
the durable exit receipt `/api/live`, `session/stop` and the recycle bin fold
into `exited`; cancellation still downgrades it. `get`, `list` and authorization only request guarded status;
they never send bind. Every probe clears its prior binding observation before
network I/O, so timeout, shutdown, vanished host or known Child exit cannot reuse
cached host data. A current matching Info may confirm the retained intent;
missing status makes lifecycle state Uncertain but preserves an already Confirmed
association; unbound, conflicting or invalid status makes the binding Uncertain. A conflicting or invalid
binding persists Uncertain, returns a typed conflict and retains the original spec.

Binding association survives Web restart in the ledger. Live terminal authority
does not: it still requires fresh guarded status from the same running host
instance. A new host instance at the same routing name does not inherit authority.
Queries about cancelled/exited instances may retain or observe their old
association, but never authorize bind or terminal use.

## Native terminal authorization after Web restart

For a `BoundTarget` whose `origin_launch_id()` is present, callers must use
`LifecycleService::authorize_native(&target)` before a native claim/attach. The
coordinator finds exactly one retained receipt matching source, host name, launch
and instance IDs, then performs fresh guarded status. It returns a private Record
only when Running, not cancelled, and Confirmed with exact target SID/UID.

No receipt, no durable intent, unsupported/invalid status, uncertain/exited state,
replacement, wrong SID/UID or a durable cancellation flag denies authorization.
An empty terminal registry after Web restart is **not** permission. Targets without
launch origin are outside this method's authority and are rejected. Callers must
still perform normal guarded terminal claim/attach and enforce same-gate retirement;
this method is a current authorization check, not an irrevocable future lease.

Cancellation retains its previous persist-before-retire-before-kill ordering.
The terminal layer retires native leases derived from the exact launch as well as
pending launch leases. Duplicate cancellation may retry local retirement, never
kill or bind. Persistent cancellation blocks derived native authorization even
when the host remains alive and the Web process restarts with an empty registry.

## Validation scope

Synthetic store tests cover durable intents, exact/old authorities, conflicting
specs and observations, explicit retry, all injected write-failure boundaries,
schema-1/2 migration, required shapes, source identity and cancellation races.
Service peers validate persist-before-call, dropped responses, lost ACK and missing
Info, ACK without a matching observation, explicit retry versus read-only recovery,
Confirmed-to-offline downgrade, unsupported scopes/capability, stale instances,
host binding without ledger intent, and cancellation protection across service
restart. These tests do not start a CLI or alter native history.
