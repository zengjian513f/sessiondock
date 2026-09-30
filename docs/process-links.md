# Shared process attribution and session resource accounting

`process-links` is the common attribution protocol and Rust library. SessionDock
hosts its node API and fleet coordinator; Node Status consumes the same node
bindings. The library does not collect CPU usage or depend on either UI.

## Identity and causality

A process incarnation is `(execution_node_id, boot_id, pid, start_ticks)`.
The execution node is always the node reporting the process, even when its
session lives elsewhere. A session is `(node_id, source, native_sid)`.

Each binding names its direct owning `session` and, when observed over SSH,
an independent `initiator`. A remote Python command without its own CLI session
is owned by its initiator. A remote CLI retains its own session identity, while
its launch initiator remains available separately. Resuming an older session
does not make that older session a newly created child.

SSH links also retain the launching process's complete `ProcessKey`, connection
tuple and first observation time. Bindings also expose a `launch_chain` (nearest
launch first), retaining earlier launch identities across additional SSH hops
and after an upstream launcher exits. An established attribution stays attached to
that process incarnation; a later connection reusing the same ports cannot
reassign old work. Native `spawned_by` is recorded only when both creation times
are known and the parent is not newer than the child. Its optional `node_id`
identifies a remote parent. Manual sidebar nesting never changes process ownership.

## Current wire contract

`GET /api/process-links` returns version 1 with:

- `node_id`, `boot_id`, `sampled_at`, `supported`;
- `bindings`: process identity, direct session, optional initiator, `launch_chain` and
  `first_observed_at`;
- `outgoing`: SSH client process, start time, full TCP tuple, owning session and inherited launch chain;
- `incoming`: process identity/start time and inherited SSH connection tuple.

An external resource sampler joins its own process observations to `bindings`
using the full process incarnation. It must validate boot and start identities;
matching a numeric PID alone is insufficient. A missing binding is unassigned,
not zero resource usage. The process list is independent of CPU activity and
therefore also covers GPU-heavy, sleeping and I/O-bound processes.

The Hub polls enabled nodes, accepts reports only under the registered node's
identity, correlates unique direct connections and publishes verified links to
the destination's authenticated node listener. Browser forwarding of
`POST /api/process-links` is refused. Node Status needs only the local read API;
it neither holds fleet credentials nor duplicates the connection matcher.

The Linux adapter reads the current user's processes, selected identity variables,
SSH connection variables and socket descriptors. It excludes detected SSH master
connections, ambiguous matches and receiver processes predating a new connection
(with two seconds of clock skew tolerance). Shared tmux ancestors do not establish
ownership. No shell command changes or environment forwarding are required.

Verified remote process links are saved in the private state directory as
`process-links.json`. Restart recovery requires the same boot and live process
incarnation. The snapshot collector can miss tasks that start and exit between
samples; it does not promise complete accounting of short-lived work. macOS and
Windows currently report `supported: false`; platform adapters are follow-up work.

## Resource collection and aggregation contract

The following defines the extension direction, not metrics already implemented.
Attribution, measurement and aggregation remain separate so SessionDock and
Node Status can consume one result without collecting or adding it twice.

Resource observations must carry the execution node, process/cgroup/socket/device
identity, observation interval, unit, collection method and availability status.
Cumulative counters and point-in-time gauges remain distinct. Collectors preserve
the underlying counters; rates are differences over measured elapsed time.
Unsupported, stale, unassigned and shared measurements never become numeric zero.

| Resource | Session display | Accounting boundary |
| --- | --- | --- |
| CPU | Logical cores in use; cumulative CPU seconds | Sum process CPU-time deltas, then divide by wall time; do not add percentages normalized to different machine sizes |
| GPU | Distinct devices in use, per-device VRAM; process utilization where available | Device identity includes execution node and GPU UUID; using a card does not mean owning its full compute capacity |
| Memory | Proportional resident memory or explicitly labelled cgroup memory | PSS apportions shared pages; RSS sums can double-count them; collection methods must remain visible and distinct |
| Local storage | Read/write throughput, cumulative bytes and operations, optionally latency | Preserve device identity and distinguish logical application I/O from block-device I/O; asynchronous writeback needs suitable attribution |
| Network | Send/receive rate and cumulative bytes | Account sockets/cgroups with lifecycle evidence; interface totals are machine context, not per-session counters |
| NFS | Read/write traffic, RPC operations, latency/retransmits where attributable | Keep mount/server identity and separate logical file operations from wire traffic; shared kernel RPCs require dedicated tracing or an explicit unassigned bucket |

Polling is useful for a first live view. Complete totals across short-lived
processes require lifecycle events or per-execution accounting groups that retain
counters after a worker exits. Linux cgroups and targeted kernel tracing are
potential collectors, not a reason to replace the session/SSH attribution protocol.
Existing tasks must not be moved into accounting groups blindly.

There are two aggregation views: **direct** (resources directly owned by a
session) and **including launched work** (its verified execution descendants).
The latter forms a union of process/resource identities before summing. Summing
each child's inclusive total again is forbidden. A sidebar display reparenting
must not rewrite causal accounting. Resumed sessions retain their historical
identity while a particular launch can belong to the current initiating workload.

Fleet results retain both totals and per-node breakdowns. CPU cores and compatible
memory/throughput measurements can be added across execution nodes. Device counts
are sets, not additive counters; NFS is a kind of network/storage activity and must
not be added again to the corresponding physical totals. Heterogeneous CPU core
counts describe occupancy, not equal performance across models.

## Validation

`python3 tests/process_links_browser.py --binary target/debug/sessiondock`
uses private synthetic process trees, two isolated nodes, a real Hub and Chromium.
It exercises remote CLI nesting, a plain remote Python process, creation-order
checks, browser write rejection, restart recovery and process identity reuse.
The Node Status resolver has a separate consumer regression for start-time checks.

Related contracts: [liveness](liveness.md#spawned_by),
[metadata](metadata.md#spawned_by), [deployment](deployment.md).
