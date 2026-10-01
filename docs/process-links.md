# Shared process attribution and session resource accounting

`process-links` is the common attribution protocol and Rust library. SessionDock
adapts native session identities and hosts the fleet coordinator. An independent
`resource-agent` system service owns collection and durable attribution when
installed; Node Status reads its local socket directly, including while
SessionDock is stopped. The node API retains a polling compatibility path. The library does not collect CPU usage or depend on either UI.

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

The Linux adapter reads the monitored user's processes, selected identity variables,
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

## Independent Linux service

`resource-agent` is a user-space systemd service, with read-only eBPF lifecycle
probes loaded by a managed bpftrace child. It does not change the kernel image,
load a kernel module, change SSH configuration, wrap commands, move workloads
into cgroups, throttle workloads, or signal them. Its own cgroup has CPU/memory
limits; stopping that cgroup stops only the collector and its tracer.
These limits do not bound probe execution charged to monitored workloads.
Per-call VFS/TCP tracing is therefore disabled by default and explicitly disabled
in the supplied service unit. `--io-events on` starts one bounded 60-second diagnostic window;
`--events off` disables both lifecycle and I/O probes. Default collection retains
lifecycle attribution, CPU, PSS, GPU and `/proc/PID/io` storage rates. Local-file,
NFS and TCP logical rates are unavailable (null) when I/O probes are off.
Do not enable them for production throughput workloads without a representative
on/off overhead measurement; cached small reads can be dominated by probe cost.

The opt-in `resource-agent` deploy target installs the root-owned binary under
`/opt/resource-agent`, a system unit and a non-secret configuration file naming
the monitored UID and existing node identity file. The local Unix socket
`/run/resource-agent/agent.sock` is mode 0600, owned by that UID; the server also
checks peer credentials. No TCP listener or new network credentials are added.
The supplied unit bounds capabilities to BPF/performance tracing, reading process
state, resource limits and socket ownership. It does not make either UI privileged.

The newline-delimited JSON requests are `health`, `report`, `resources`, `probe`,
`catalog` (native session identities and process owners), and `publish` (verified
SSH links), encoded as `{ "op": "report" }` or `{ "op": "catalog", "data": ... }`.
Responses are `{ "ok": true, "result": ... }` or an explicit error. SessionDock
refreshes its catalog in the background, while the service retains confirmed
process bindings through application outages and service restarts within a boot.
Catalog and link identities are local-node/boot/process scoped. A missing remote
collector creates a gap in visibility, never a zero-resource observation or an
SSH failure. Legacy nodes can still provide polling attribution.

The resource endpoint retains cumulative CPU seconds, RSS and `/proc/PID/io`
counters and adds sampled rates, PSS, NVIDIA compute-process GPU UUIDs and
framebuffer memory, and read-only application I/O probes. Per-process metric
objects carry `value`, `status` and a coverage `reason`; unavailable values are
null, never zero. The first counter interval is warming up. Rates require the
same boot and exact process incarnation. CPU is occupied logical cores, not a
percentage of the entire machine. GPU count is the distinct set of resident
compute devices per execution machine, not exclusive allocation or GPU compute
utilization. Graphics-only work and MPS worker attribution are not covered by
the NVIDIA compute query. PSS failure never substitutes RSS. PSS uses a separate low-frequency cache
(about 30 seconds) so expensive page-table scans cannot stall CPU/I/O sampling;
metric timestamps retain the oldest contributing sample through aggregation.
GPU queries also run independently, about every 10 seconds; values older than
30 seconds are unavailable. PSS cache entries expire after 60 seconds.

Local file and NFS read/write rates are successful synchronous VFS application
bytes, distinguished by filesystem; they are not physical disk operations or
NFS RPC traffic. These probes exclude mmap, io_uring and splice. TCP rates are
successful application send/receive bytes, excluding MSG_PEEK, UDP, retransmits
and NFS kernel RPC traffic. Coverage is explicitly partial. The separate
`proc_storage_*` rates retain `/proc/PID/io` storage accounting without mixing
it with VFS logical bytes. Kernel map limits, event loss and unavailable probes
remain visible; this is not a complete historical billing ledger.

`GET /api/session/resources?uid=...&scope=direct|inclusive` resolves the actual
native session identity from the inventory, then reports totals and each
execution machine. Hub reads fan out to discover verified session bindings,
but only the owning machine and verified execution participants appear in the
view or its totals. Unrelated machines are hidden even when offline. A verified
participant remains visible as unknown during an outage while the Hub retains
that observation; changing API direct/inclusive scope does not change participation.
The resource drawer defaults to inclusive accounting, with scopes labeled
“仅当前会话” (direct) and “包含子会话” (inclusive), independent of machine location;
metric definitions, sampling cadence and partial coverage appear in Chinese tooltips. Inclusive accounting takes the union of
verified process identities, never sidebar nesting or summed child totals.
GPU UUIDs are deduplicated within each machine. The local agent also publishes
`sessions` using the same direct aggregation for Node Status consumers.
Lifecycle events preserve inherited attribution after a parent exits. Polling
repairs the live snapshot every two seconds; short-lived processes and SSH
connections can still have gaps. Missing coverage does not affect workloads.

The Hub still coordinates new cross-machine matches using node APIs. Existing
bindings and local collection survive application/Hub outages; discovering new
remote links requires both endpoint reports and a working coordinator. Kernel
probes are local to each machine and do not remove that requirement.

Validation: `tests/resource_agent_suite.py`, `tests/resource_agent_deploy.py`,
and `tests/process_links_browser.py --with-agent` cover independent lifetime,
restart recovery, no workload termination, PID reuse, unavailable metrics and the
browser-visible cross-machine relation. Live BPF validation additionally checks
that a short child fork and exit are observed without changing its command.


## Temporary I/O diagnostics

The session resource drawer provides “探测 60 秒” and “停止探测”. The Hub sends
`POST /api/session/resources/probe` (`uid`, `scope`, `enabled`) only to the
session owner and verified execution participants. Each machine returns its own
state and remaining seconds; partial failures are visible. Direct machine
`POST /api/resources/probe` requires authenticated Hub access. Local consumers,
including Node Status, use the same private socket request
`{"op":"probe","data":{"enabled":true}}` (false stops).

The lease lasts at most 60 seconds from acceptance and repeated enable requests
do not extend it. The agent owns the timer independently of page polling, Hub
availability and the process sampler. The helper also has a 60-second watchdog,
a parent-death signal and a kernel-side accounting deadline. Stop/expiry kills
and reaps only the helper and closes its unpinned BPF links/maps; monitoring
never signals workloads. Agent restart defaults back to off. Diagnostic state
is published in `resources.diagnostic` and each session resource node.

The I/O BPF object is compiled at build time (Clang BPF backend) and embedded in
the executable. Targets need `libbpf.so.1`, compatible BTF and the existing BPF
capabilities, but no runtime compiler for I/O diagnostics. Lifecycle attribution
still uses its existing bpftrace collector. Loading or attachment failure is an
explicit unavailable result and closes partial attachments.

Counters aggregate in kernel maps instead of sending an event for every I/O.
The temporary probe covers the monitored user's processes on each participating
machine, so other session views share that machine's diagnostic window. Turning
it off from one view stops it for all views on that machine. Probe execution can
still affect small-I/O throughput. CPU, PSS, GPU and proc storage accounting
remain available while diagnostics are off; unavailable diagnostic rates are
null, never stale values presented as live zeroes.
