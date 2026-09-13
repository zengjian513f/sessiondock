# Local developer runbook

Run the Rust server against **synthetic** repository data. Documentation only;
native history stays read-only. Every `SESSIONDOCK_*` name below is parsed in
`crates/sessiondock/src/config.rs`.

## Never do this

From [AGENTS.md](../AGENTS.md#scope-and-boundaries): no non-loopback binds;
no production hosts, registries, queues, or active CLI sessions; no real CLI
homes for roots, ptyhost `--dir`, or indexes; real CLIs under test always with
the cheapest model at low effort (`claude-haiku-4-5-20251001` / `gpt-5.6-luna` / `grok-4.6`)
unless the user authorizes more; no overlap
among `legacy-web`, native roots, ptyhost, state, delivery, lifecycle, launcher
JSON, Codex index, and file roots; no ptyhost without an explicit development
`--dir`; no committing addresses, personal paths, credentials, runtime data,
builds, or local env files.

## 1. Build

```sh
cargo build --release -p sessiondock
cargo build -p ptyhost
```

Binaries: `target/release/sessiondock` and `target/debug/ptyhost`. Always pass an isolated `--dir` to ptyhost; see [session-host.md](session-host.md).

## 2. Synthetic corpus

[`tests/fixture_gen.py`](../tests/fixture_gen.py) writes `OUTDIR/{claude,codex,grok}` (no CLI, no network).

```sh
python3 tests/fixture_gen.py .runtime/dev/corpus --sessions 3 --records 50 \
  --images 4 --forks 1 --agents 1 --print-env
```

`outdir` is required (`--force` if nonempty). Defaults: `--sessions 3`,
`--records 200`, `--images 0`, `--forks 0`, `--agents 0`,
`--sources claude,codex,grok`, `--seed 1`. `--print-env` prints
`SESSIONDOCK_CLAUDE_ROOT`, `SESSIONDOCK_CODEX_ROOT`, `SESSIONDOCK_GROK_ROOT`,
`SESSIONDOCK_BIND=127.0.0.1:8741`, and `SESSIONDOCK_WEB_DIR` → `legacy-web`.

## 3. Minimal environment

Eval those export lines, or set them yourself. Bind **must** be loopback.

```sh
export SESSIONDOCK_BIND=127.0.0.1:8741
export SESSIONDOCK_WEB_DIR=legacy-web
# plus the three *_ROOT directories from the corpus
./target/release/sessiondock
```

Unset optional variables leave those capabilities off; empty directory values are rejected.
The page title, brand, `/api/meta.hostname` and `/api/nodes` use the system
host name (Linux `/proc/sys/kernel/hostname`, Python's `socket.gethostname()`);
`SESSIONDOCK_HOSTNAME=<name>` overrides it (`SessionDock` only when neither is
available). `--check-config` prints the effective `hostname=` and the
`debug_runs=` registry path (`<state dir>/debug-runs.json`, see
[read-model.md](read-model.md#debug_run-视图python-agenthubdebug_runspy)).

`./target/release/sessiondock --check-config` runs exactly the startup
validation (`Config::from_env`) on the current `SESSIONDOCK_*` values, prints
`config=ok` plus one `key=value` line per setting (`(unset)` when off) and
exits 0 — or prints the same `Error: …` startup would and exits 1 — without
opening a ledger, binding a port or starting a CLI. Use it before a real
start; `tests/cutover_preflight.py` wraps it together with the environment
checks of the cutover checklist, and `tests/check_config_suite.py` pins the
validation rules it applies.

## 4. Optional isolated features

Use **sibling** directories under one private tree (example `.runtime/dev/`).
Unix delivery/lifecycle/audit/ptyhost dirs: already exist, absolute, mode
`0700`, no symlink ancestors. None may equal, contain, or lie under another
configured path (including `legacy-web`).

**Preferences — `SESSIONDOCK_STATE_DIR`.** Existing dedicated directory; the
store does not create it or migrate Python state. See [metadata.md](metadata.md#directory-and-schema).
The debug-run registry `debug-runs.json` (Python's format, written by the
test tooling) may sit beside the metadata; `tests/meta_import.py` copies
Python's over.

**Search text — `SESSIONDOCK_SEARCH_CACHE_DIR`.** Its own existing 0700
directory (never inside the state directory, which the metadata store keeps
to itself); `SESSIONDOCK_SEARCH_CACHE_BYTES`, `SESSIONDOCK_SEARCH_WORKERS` and
`SESSIONDOCK_SEARCH_WARMUP` tune it ([read-model.md](read-model.md#搜索)).
Unset keeps the cache memory-only (64 MiB) and nothing is warmed.

**Delivery — `SESSIONDOCK_DELIVERY_DIR`.** Existing empty private directory.
Initialize without starting Web, then export the same path:

```sh
./target/release/sessiondock --initialize-delivery "$PWD/.runtime/dev/delivery"
```

No implicit ledger init. [Directory rules](delivery-configuration.md#directory-requirements).

**Ptyhost + launcher.** `SESSIONDOCK_PTYHOST_DIR` is the isolated host
directory and must **equal** `host_dir` in the launcher JSON. Creation also
needs both `SESSIONDOCK_LIFECYCLE_DIR` and `SESSIONDOCK_LAUNCHER_CONFIG`
([lifecycle HTTP](lifecycle-http.md#explicit-configuration-and-startup)):

```sh
./target/release/sessiondock --initialize-lifecycle "$PWD/.runtime/dev/lifecycle"
```

Launcher JSON: absolute existing file, Unix mode `0600`, one hard link, ≤64 KiB.
Reuse the schematic examples in [lifecycle-launcher.md](lifecycle-launcher.md#private-configuration)
(never real CLI homes). Replace `host_binary` with the built ptyhost **absolute**
path and `host_dir` with `SESSIONDOCK_PTYHOST_DIR`. Keep `example-adapter-v1` as
the free-shell adapter and schema 2 `claude-cli-v1` as a fake CLI profile.
`cwd_roots` must exist and stay outside every other configured path.

```json
{
  "schema": 2,
  "host_binary": "/opt/example/bin/ptyhost",
  "host_dir": "/var/lib/example/hosts",
  "cwd_roots": ["/srv/example/work"],
  "adapters": [{"id": "example-adapter-v1", "source": "codex",
    "executable": "/opt/example/bin/verified-adapter",
    "args": ["fixed-adapter-argument"], "env": {"TERM": "xterm-256color"}}],
  "profiles": [{"id": "claude-cli-v1", "source": "claude",
    "executable": "/opt/example/bin/claude",
    "args": ["--settings", "/etc/example/claude-bridge-settings.json"],
    "new_args": ["--session-id", "{session_id}"],
    "resume_args": ["--resume", "{sid}"],
    "env": {"PATH": "/usr/bin:/bin", "HOME": "/home/example"},
    "env_remove": [], "cwd_roots": ["/srv/example/work/claude"]}]
}
```

`/opt/example` paths are documentation only. For a working free-shell adapter,
point `executable` at a private POSIX shell you control, never a model CLI. [CLI profiles](lifecycle-launcher.md#cli-profiles-batch-24-schema-2).

**Audit — `SESSIONDOCK_AUDIT_DIR`.** Existing private `0700` directory. Unset
keeps `audit:false` and route `501`. [Directory](diagnostics.md#directory).

**File reads — `SESSIONDOCK_FILE_ROOTS`.** Unix `:`-separated list of 1–16
existing directories, disjoint from static/native/state/host/delivery/lifecycle
and the Codex index. Session-reference scoped; cwd is not a grant.
[files.md](files.md#module-interface).

**Codex names — `SESSIONDOCK_CODEX_INDEX`.** Existing absolute
`session_index.jsonl`. Requires `SESSIONDOCK_CODEX_ROOT`. Must sit **outside**
every configured root (a sibling file is fine). [codex-names.md](codex-names.md).

## 5. Open the page

When the process prints `SessionDock: http://127.0.0.1:8741`, open that URL.
HTML comes from `legacy-web/` (no Node build). The snapshot injects `meta[name="agenthub-capabilities"]`.

## 6. Banner and capabilities

[`legacy-web/capabilities.js`](../legacy-web/capabilities.js) parses that meta
tag. `allows(name)` is true unless the value is exactly `false`. Malformed JSON
forces `read_only:true` and `configuration_error:true`. `#backend-notice` is
shown when `read_only` is set: native records are read-only; unmigrated features
stay unavailable; `metadata` saves stars/visibility to the state directory;
`terminal` allows manual input on verified consoles (`terminal_create` means
controlled create is configured; reliable send stays off); without `live`, run
state is unknown (default `live` is false).

Defaults on: `sessions`, `watch`, `search`, `media`, `media_lazy`,
`history_pages`, `media_continuation`. Defaults off: `live`, `terminal`,
`outbox`, `audit`, `files`, `mutations`, `hub`, `media_remote`. Configuring a
directory flips `metadata`, `files`, `audit`, `outbox_read`, and the `terminal*`
flags. `outbox` (send) stays false.

## 7. Watch a session

If [`tests/sse_probe.py`](../tests/sse_probe.py) is present, copy a UID from the page and:

```sh
python3 tests/sse_probe.py --base http://127.0.0.1:8741 --uid 'codex:…' --duration 30
```

Loopback only. Options: `--agent`, `--max-events`, `--json`, `--verbose`. It GETs `/api/messages/{uid}?window=1`, then `/api/watch`.

## 8. Sample memory

[`tests/rss_watch.py`](../tests/rss_watch.py) reads `/proc` only; it never signals the process.

```sh
python3 tests/rss_watch.py --pid "$SERVER_PID" --interval 0.5 --duration 60 --children
```

Optional `--csv PATH` and `--quiet`.

## 9. Shut down

SIGINT or SIGTERM. The process cancels admission, drains lifecycle/delivery
locks and audit (best-effort), then exits. It does **not** kill established
ptyhost children. Inspect leftover hosts only with
`target/debug/ptyhost --dir <isolated-dir> list` against that same development directory.
