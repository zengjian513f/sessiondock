# Local developer runbook

Run SessionDock against synthetic or explicitly authorized data. Ordinary reads
preserve native histories. Explicit clone, move and trash operations change only
the confirmed group/files under their contracts; test those in a synthetic tree.

## Never do this

Do not expose the unauthenticated ordinary listener directly, commit credentials
or runtime data, or point destructive test cases at production paths. Use an
explicit ptyhost directory and coordinate use of the shared Cargo target directory.

## 1. Build

```sh
cargo build --release -p sessiondock
cargo build -p ptyhost
```

The `sessiondock` package also builds `sessiondock-hub`; use `--bin sessiondock-hub`
when selecting that binary alone for Hub federation.

## 2. Synthetic corpus

Point the read roots at the checked-in synthetic fixtures (see the README):
`crates/sessiondock/tests/fixtures/{claude,codex,grok}`. Copy them into a
private directory first when exercising writes.

## 3. Minimal environment

```sh
export SESSIONDOCK_BIND=127.0.0.1:8741
export SESSIONDOCK_WEB_DIR=legacy-web
# export the desired native read roots
./target/release/sessiondock
```

`sessiondock --check-config` parses and reports the effective configuration
without binding a listener or starting a CLI. The hostname defaults to the system
hostname and can be overridden with `SESSIONDOCK_HOSTNAME`.

## 4. Optional private runtime services

Use a private development tree for synthetic ptyhost, lifecycle, audit,
trash and bug-report data. The runtime follows each module's documented format;
it does not impose a blanket disjoint-root or 0700 policy on otherwise valid
layouts.

**Metadata — `SESSIONDOCK_STATE_DIR`.** Metadata reloads on every operation.
Missing, damaged or unsupported data behaves as empty; external edits are
allowed. See [metadata.md](metadata.md).

**Search text — `SESSIONDOCK_SEARCH_CACHE_DIR`.** Optional persistent cache;
bodies are built on search requests, with no startup or periodic scan. Cache
size and workers affect performance and eviction, not which valid
search input is accepted. See [read-model.md](read-model.md#搜索).

**Ptyhost and launcher.** Configure `SESSIONDOCK_PTYHOST_DIR`, lifecycle storage
and a launcher file when testing create/resume/takeover/stop. Launcher profiles
choose executable and fixed arguments; HTTP supplies session identity and cwd.
External sessions use the same observed-instance confirmation path.

**Files and attachments.** File reading is available without a separate root
grant. Writes are enabled with terminal operation. Test uploads and mutations
inside the synthetic tree; see [files.md](files.md).

**Hub.** Configure the node listener as a complete peer/protocol/token set. The
Hub registers literal IP HTTP or HTTPS URLs; HTTPS uses the system trust store.

**Audit and bug reports.** Configure their stores only when exercising persisted
diagnostics. Their absence does not disable unrelated file or attachment APIs.

## 5. Open the page

Open the loopback URL printed by SessionDock. `legacy-web/` is served directly.

## 6. Banner and capabilities

The injected capabilities describe current runtime support. Conversation SEND
needs metadata, terminal transport, lifecycle and file writes. File reads are available; file writes track
terminal operation. A false capability should reflect a truly unavailable
service rather than an old migration stub.

## 7. Watch a session

```sh
curl -N 'http://127.0.0.1:8741/api/watch?uid=codex:…'
```

## 8. Sample memory

```sh
grep -E 'VmRSS|VmHWM' "/proc/$SERVER_PID/status"
```

## 9. Shut down

SIGINT or SIGTERM stops admission and drains service work. It does not kill
independent ptyhost children. Inspect a test host with:

```sh
target/debug/ptyhost --dir <explicit-test-dir> list
```
