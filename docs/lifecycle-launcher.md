# Lifecycle launcher

`lifecycle::launcher` turns a server-configured source profile into the argv
passed to `ptyhost`. Browser requests choose `claude`, `codex`, or `grok` and a
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
An exited shell row stays listed until 删除, replaying its recording read-only
([terminal records](terminal-records.md)); terminal scrollback itself is
bounded and held in the host's memory, and an exited shell has no resume
operation. Shell-managed history files still follow the selected machine's
shell configuration.

The host command line is `--dir <host dir> run --name <host name> --cwd <cwd>
--meta <json> [--no-record] -- <argv>`. Only a shell session's host records its
terminal ([terminal records](terminal-records.md)): every other source gets
`--no-record`, because an agent session's record is its native transcript and
the recording would be a second copy of the same output with no reader. A node
keeps running the host binary its configuration names, which may predate
recordings and rejects the unknown option with status 2 before the CLI starts;
before every launch the launcher probes the host (`--dir <host dir>/.probe
--no-record list`, an empty private directory, so no session file is read or
removed) and such a host, which never records anyway, is simply not told.

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
- Grok new: `--session-id <generated UUID>`; resume: `--resume <sid>`.

When a configured template contains an exact `{session_id}` or `{sid}` argument,
the launcher substitutes it. Otherwise it appends the command above.
Fixed profile arguments stay before those identity arguments.

Managed Codex profiles append `-c check_for_update_on_startup=false` for new,
resume and report-worker launches. An interactive startup update exits Codex
before native session creation, abandoning the pending session. Update Codex
from an external terminal instead; this per-invocation override leaves user
configuration and model/effort defaults untouched. Fixed-argv adapters are
unchanged. See the [official configuration reference](https://developers.openai.com/codex/config-reference).

The child inherits the service environment, then applies `env` and
`env_remove`. Session-lineage variables are cleared:
`CLAUDE_CODE_SESSION_ID`, `CODEX_COMPANION_SESSION_ID`,
`GROK_SESSION_ID`, `CODEX_THREAD_ID`, `CODEX_SESSION_ID`, and `CLAUDE_PID`.
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

Default check is `python3 tests/check_config_suite.py` and
`python3 tests/lifecycle_http_suite.py` when the launcher argv or environment
changes. Do not run crate unit tests unless the user asks.
