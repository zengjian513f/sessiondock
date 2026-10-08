# Lifecycle launcher

`lifecycle::launcher` turns a server-configured source profile into the argv
passed to `ptyhost`. Browser requests choose `claude`, `codex`, `grok`, `opencode`, or `agy` and a
working directory; the `shell` source opens the selected node's interactive
terminal (labelled SSH in the browser). Executable, fixed arguments, and environment still come
from server configuration, so request JSON cannot become an arbitrary command.

## Configuration compatibility

The launcher JSON contains `host_binary`, `host_dir`, and either `adapters` or
`profiles`. Existing deployments may keep `schema`. Unknown JSON
members and duplicate environment-map keys are accepted by normal serde mapping.
Adapter/profile IDs use the persisted identifier alphabet and do not need a
version suffix.

Existing launcher files automatically gain a fixed `shell` adapter: Unix uses
the service's usable `SHELL`, falling back to `/bin/sh`, with `-i`; Windows uses
`COMSPEC`, falling back to the system `cmd.exe`. An explicit `shell` adapter or
profile overrides this default. Shell launches have `launch_kind: fixed`, no
native SID/UID, and no resume or native binding. They remain reopenable terminal
rows without waiting for an AI conversation record. A node with only host
configuration can offer this terminal without an AI CLI installation.
An exited shell row stays listed until 删除, showing its final screen read-only
([terminal final screen](terminal-final-screen.md)); terminal scrollback itself is
bounded and held in the host's memory, and an exited shell has no resume
operation. Shell-managed history files still follow the selected machine's
shell configuration.

The host command line is `--dir <host dir> run --name <host name> --cwd <cwd>
--meta <json> [--no-record] -- <argv>`. Only a shell session's host keeps its
final screen ([terminal final screen](terminal-final-screen.md)): every other
source gets `--no-record`, because an agent session's record is its native
transcript. A node keeps running the host binary its configuration names, which
may predate the option and rejects it with status 2 before the CLI starts;
before every launch the launcher probes the host (`--dir <host dir>/.probe
--no-record list`, an empty private directory, so no session file is read or
removed) and such a host, which keeps nothing after exit anyway, is simply not told.

The host and CLI executable paths must be usable absolute executable files, and
the host directory must be usable by `ptyhost`. Executable symlinks are
resolved. The launcher checks them when the configuration loads and again
before every spawn, as they are at that moment: a CLI that updated itself since
the service started (the Windows Claude installer overwrites `claude.exe` in
place; the Unix installer re-targets `~/.local/bin/claude`) launches its
current file without a service restart. Only a path that no longer names an
ordinary executable file is refused (`invalid_launch`).

Profiles provide fixed `args`, optional legacy `new_args` and `resume_args`,
`env`, and `env_remove`. Configured argument templates remain compatible, but
missing identity templates no longer disable a source:

- Claude new: `--session-id <generated UUID>`; resume: `--resume <sid>`.
- Codex new: no assigned SID; resume: `resume <sid>`.
- Agy new: no assigned SID; default resume argv: `--conversation <sid>`.
  Its read-only native mirror and conversation DB fd evidence provide the
  pending-to-native identity path. Model choice uses `--model`; optional effort
  uses `--effort`. The real CLI browser test covers composer input, menus and
  stopped-session continuation. See [Agy](agy.md).
- Grok new: `--session-id <generated UUID>`; resume: `--resume <sid>`.
- OpenCode new: `--session <assigned ses_… id>`, after creating that session
  through `opencode api session.create` ([OpenCode](opencode.md)); resume:
  `--session <sid>`.

When a configured template contains an exact `{session_id}` or `{sid}` argument,
the launcher substitutes it. Otherwise it appends the command above.
Fixed profile arguments stay before those identity arguments.

OpenCode is an AI CLI source like the others: native UID `opencode:<hash>`,
`new_assigned` launches whose id the launcher creates in OpenCode first,
a resume profile, and no final screen. Its sessions come from
OpenCode's SQLite store through SessionDock's mirror ([OpenCode](opencode.md)).
A node offers OpenCode only when its launcher configuration names exactly one
`opencode` profile, for example `{"id": "opencode-cli-v1", "source":
"opencode", "executable": "<absolute path>", "args": ["opencode"],
"resume_args": ["--session", "{sid}"]}`.

Managed Codex profiles append `-c check_for_update_on_startup=false` for new,
resume and report-worker launches. An interactive startup update exits Codex
before native session creation, abandoning the pending session. Update Codex
outside managed sessions instead (the machine settings below, a scheduled
`codex update`, or an external terminal); this per-invocation override leaves
user configuration and model/effort defaults untouched. Fixed-argv adapters are
unchanged. See the [official configuration reference](https://developers.openai.com/codex/config-reference).

## Client versions and manual updates

The Machines settings tab has an AI client matrix below the machine list: one
row per enabled machine, one column per client installed on at least one of
them. A cell shows only the installed version and an `↑` update button:
highlighted when a newer version is known, plain when the version is the
newest, and faded while that is unknown; the newest version, the lookup state
and the last update's outcome are in the cell's tooltip. A client is judged
against the highest of every machine's newest version and installed version
for that client, so a machine whose own lookup failed, or that runs an older
build than another machine, still shows as upgradable; it shows as newest only
when some machine's lookup answered. `无` means that machine lacks the
client; a whole row reads `…` while loading and `离线` when the machine is
offline or cannot be read (reason in the tooltip). An update fails only when
its command exits non-zero, times out or cannot start.
`GET /api/clients` runs each profile's executable with its fixed `args` plus
`--version` (10 s each, in parallel, under lifecycle admission) and answers
`{clients:[{id, source, version?, detail, installed, latest?, latest_state?, update?}]}`:
`detail` is the first output line and `version` its first `1.2.3`-shaped word.
Installed-version answers, including missing commands, are cached per profile
for 60 s after a probe finishes. Concurrent requests share one probe per
profile; polling and reopening settings reuse its answer. An expired answer
is refreshed on the next request. Background version checks (including startup
availability and model-catalog checks), latest-version lookups and updates use
Windows `CREATE_NO_WINDOW`, so these commands do not create desktop consoles.
A profile whose command is missing (the picker's 126/127 rule) is
`installed:false` and gets no cell. `latest` is the newest version on the
channel the CLI's own updater follows, looked up with `curl` in the profile's
environment (so its proxy applies): Claude's npm dist-tag named by
`autoUpdatesChannel` in its `settings.json` (default `latest`), Codex's npm
`latest`, OpenCode's `opencode.ai/update/api/latest/cli/npm`, and Grok's own
`update --check --json`. Agy reads `version` from its official updater's
platform manifest (`manifests/{os}_{arch}{suffix}.json`; macOS uses `darwin`,
x86_64 uses `amd64`, aarch64 uses `arm64`, and Linux musl adds `_musl`) at
`antigravity-cli-auto-updater-974169037036.us-central1.run.app`. This is a
read-only lookup; Agy manual updates use `agy update`. These paths are
implemented; local and Hub browser tests cover update success/failure using synthetic CLIs ([Agy](agy.md)).
The lookup crosses the network, so it runs on its own
thread and the answer never waits for it: `latest` is the last answer that
arrived (a failed lookup never erases it), and `latest_state` is `pending`
while a lookup runs or `failed` when the last one failed. A lookup starts when
the answer is older than 10 minutes, or 1 minute after a failure; curl
connects within 4 s, gives up after 8 s and retries twice. The page keeps
polling every 2 s while any lookup is pending, and compares versions by
numeric parts.

`POST /api/clients/update {id}` claims that profile's update slot (404
`unknown_client` for an unknown or shell ID, 409 `client_update_running` while
one runs) and answers `{started:true}` at once. The update runs on its own
thread, holds no admission, and executes the profile's executable, `args` and
environment plus `update`: Claude, Codex, Grok and OpenCode all install without
asking under that name. Stdin is closed, the working directory is the profile's
`HOME`, the whole process group is killed after 600 s, and output pipes left
open by a background process are read for at most 5 s after exit. `before` and
`after` come from `--version` probes run just before and after the update;
both replace the installed-version cache immediately, even if the update fails.
`ok` comes from its exit status alone. The latest outcome per profile
(`running, started_at, finished_at, ok, code, before, after, output`, where
`output` is the last 4000 characters of stdout then stderr with terminal
escapes and carriage-return redraws removed) stays in memory until the service
restarts. The page polls every 2 s while an update runs and reports the
finished result in the Machines note; the Hub reaches each machine through the
explicit `/api/nodes/<id>/api/clients…` proxy. Running sessions keep the
binary they started with; new launches resolve the executable again.

The child inherits the service environment, then applies `env` and
`env_remove`. Session-lineage variables are cleared:
`CLAUDE_CODE_SESSION_ID`, `CODEX_COMPANION_SESSION_ID`,
`GROK_SESSION_ID`, `CODEX_THREAD_ID`, `CODEX_SESSION_ID`, `CLAUDE_PID`, and
Claude Code's in-session markers `CLAUDECODE`, `CLAUDE_CODE_CHILD_SESSION`,
`CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_CODE_SESSION_ATTENDED`, `CLAUDE_CODE_EXECPATH`,
`CLAUDE_CODE_MESSAGING_SOCKET` and `CLAUDE_CODE_MESSAGING_TOKEN` (an inherited
`CLAUDE_CODE_CHILD_SESSION` turns transcript saving off, so a service started
from inside a Claude Code session would launch Claude sessions with no native
record). Other `CLAUDE_CODE_*` settings pass through.
Ordinary values such as `TMUX`, custom variables, system paths, proxy settings,
and provider credentials are accepted. Operating-system NUL and environment
name rules still apply.

## Working directories

Working directories have no configured authorization roots. Creation accepts
any absolute path that user expansion and path resolution can resolve. `~user`
uses the operating-system user database on Unix and Windows' user-profile
convention. Existing directory symlinks are followed and the canonical directory
is stored. A missing path produces `needs_create`; only an explicit
`create_cwd: true` retry creates it and its parents. A file or inaccessible path
is rejected by the filesystem.

Directory completion also has no configured root gate. It accepts absolute
paths and current-user `~/`, preserves browser spelling, follows directory
symlinks, hides dot entries until `.` is typed, sorts case-insensitively and
then by original spelling, and returns at most 50 rows (default 24). The
4096-character completion-input check remains. Empty, relative, missing,
inaccessible, and `~other` completion inputs return an empty list.

## Launch identity

Every launch carries a generated launch ID and instance ID in immutable host
metadata. Claude and Grok new sessions also carry their generated UUID. Codex
new sessions remain pending until native records identify them. Resume resolves
the SID and UID from the frozen native catalog; the client cannot supply a SID
or argv.

A bug-report worker launches the source's one configured CLI through the same
selection as `term/create`; there is no worker-specific profile table or model
policy (a leftover `bug_report_profiles` key is ignored).

## Validation

Default check is `python3 tests/lifecycle_browser.py` and
`python3 tests/lifecycle_cli_browser.py` when the launcher argv or environment
changes. The repository has no unit tests (removed on 2026-10-06).
