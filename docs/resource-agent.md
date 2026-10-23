# Optional Linux resource collector

SessionDock includes the `process-links` library for process attribution. It does
not require Labdesk, FileDock, Node Status or a resource collector. A normal
`cargo build --release` builds the default SessionDock package without compiling
BPF code or requiring Clang, kernel tracing privileges or root.

The separate `resource-agent` executable adds local process/resource sampling and
kernel lifecycle evidence on Linux. SessionDock retains polling attribution when
the collector is missing or stopped. Missing resource measurements are shown as
unavailable, never as zero usage. Cross-machine attribution additionally requires
the optional Hub and authenticated node listeners.

## Connect a standalone SessionDock instance

Set `SESSIONDOCK_NODE_ID_FILE` to a persistent private file outside the source
checkout. SessionDock creates an identity when that file does not yet exist.
The collector must read the same file. This local identity does not enable a
network listener and needs no Hub token or peer configuration.

```sh
install -d -m 700 "$HOME/.local/state/sessiondock"
export SESSIONDOCK_NODE_ID_FILE="$HOME/.local/state/sessiondock/node-id"
export SESSIONDOCK_RESOURCE_AGENT_MODE=auto
export SESSIONDOCK_RESOURCE_AGENT_SOCKET="$HOME/.local/state/sessiondock/resource-agent.sock"
```

Start SessionDock with these variables and your normal CLI-root configuration
before starting the collector. Keep the node ID stable across restarts. Do not
copy it to a different machine.

`SESSIONDOCK_RESOURCE_AGENT_MODE` accepts `auto` (default) and `off`. `auto`
connects to an available collector only when a local node identity is configured;
it never starts, installs or elevates one. `off` makes no collector requests and
does not run the background catalog refresh. Socket lookup, when enabled, is:

1. `SESSIONDOCK_RESOURCE_AGENT_SOCKET`;
2. `RESOURCE_AGENT_SOCKET`;
3. `/run/resource-agent/agent.sock`.

If no collector is needed, leave the node identity and socket unset, or set the
mode to `off`. Session listing, history, terminals and local process polling
remain available. If a collector appears later, `auto` reconnects without
requiring the Hub.

## Build the collector separately

Build on Linux with a Rust toolchain supporting edition 2024, Clang with the BPF
backend and Linux userspace headers. The distribution packages commonly named
`clang` and `linux-libc-dev` provide these build inputs on Debian-family systems.

```sh
cargo build --release -p resource-agent
```

The build invokes `clang` by default. Set `BPF_CLANG` to a different executable
when necessary. `BPF_INCLUDE_DIR` overrides additional header directories using
the platform path separator (colon on Linux). Without this override, the build
checks `/usr/include/<target-multiarch-triple>` as well as Clang's normal include
search paths. Cross-compilation needs headers for the intended target.

```sh
BPF_CLANG=clang-18 BPF_INCLUDE_DIR=/opt/linux-headers/include cargo build --release -p resource-agent
```

The I/O BPF object is embedded at build time. Runtime I/O diagnostics need
`libbpf.so.1`, compatible kernel BTF and tracing capabilities, but do not need a
runtime compiler. Lifecycle tracing uses `bpftrace`. Missing kernel features,
devices or permissions produce partial/unavailable measurements; they do not
prevent the main SessionDock application from running.

## Run without kernel probes

A collector can first run under the same user as SessionDock, with an explicit
private socket and state file. This starts a foreground process; use your normal
service manager for a persistent installation.

```sh
target/release/resource-agent \
  --uid "$(id -u)" \
  --node-id-file "$SESSIONDOCK_NODE_ID_FILE" \
  --socket "$SESSIONDOCK_RESOURCE_AGENT_SOCKET" \
  --state "$HOME/.local/state/sessiondock/resource-agent.json" \
  --events off --io-events off --memory-bandwidth off
```

This mode uses the process information the user may read. It cannot recover
short-lived processes that exit between polls. Kernel tracing and hardware
memory-bandwidth monitoring require a separately configured privileged service;
the web application itself remains unprivileged.

The cumulative usage ledger beside the state file (`resource-agent.usage.json`
in this example) contains session identities, titles and resource measurements.
The collector writes it and its temporary file with owner-only permissions
(`0600`), independent of the process umask, and tightens existing ledger
permissions when opening it. Keep the containing directory private as above.

## Install an optional system service

The supplied [systemd unit](../deploy/resource-agent.service) installs separately
from SessionDock. After building the collector, an administrator can install it:

```sh
sudo install -D -m 755 target/release/resource-agent /opt/resource-agent/bin/resource-agent
sudo install -m 644 deploy/resource-agent.service /etc/systemd/system/resource-agent.service
```

Create `/etc/resource-agent.env` containing the numeric UID to monitor and the
absolute path of that user's existing node identity file:

```ini
MONITOR_UID=1000
NODE_ID_FILE=/var/lib/sessiondock/node-id
```

Replace both example values with your installation's values. The identity file
is the same persistent file used by SessionDock, not a second generated ID.
Then run `sudo systemctl daemon-reload` and
`sudo systemctl enable --now resource-agent.service`.

The unit uses a local socket restricted to the monitored UID and checks Unix
peer credentials. It adds no TCP listener or network token. Its capability
bounding set permits tracing, process inspection and socket ownership; it does
not grant privileges to SessionDock or its frontend.

Boot-scoped connection evidence survives collector restarts and is not truncated.
The supplied unit therefore has no fixed memory ceiling: a ceiling smaller than
the retained state can cause an OOM restart loop. State loading and saving use
buffered streaming to avoid additional whole-file buffers. CPU pacing and the
unit's CPU quota still apply; this setting does not change monitored workloads.

The supplied unit starts with I/O probes off and memory-bandwidth monitoring on.
Memory bandwidth uses monitor-only resctrl groups where supported; it never
changes allocation, affinity or cgroups. Use a local systemd override to turn it
off if unwanted. Temporary I/O probes remain explicit, time-limited diagnostics;
they are not required for CPU, PSS, GPU or process-storage sampling. Stopping the
collector leaves monitored workloads running and returns resource displays to
unavailable. Removing the collector does not require uninstalling SessionDock.

## Validate the integration

[The process-links Chromium suite](../tests/process_links_browser.py) runs with
private synthetic process trees and temporary session roots:

```sh
python3 tests/process_links_browser.py --binary target/debug/sessiondock
python3 tests/process_links_browser.py --binary target/debug/sessiondock --with-agent
```

The first command checks standalone identity, missing/disabled collectors and
browser resource controls. The second additionally requires a separately built
`target/debug/resource-agent` and checks collection without a Hub listener,
cross-node attribution, collector lifetime, restart and process identity reuse.
Neither command uses production session roots or a production collector socket.
