# Lifecycle launcher

`lifecycle::launcher` turns a server-configured source profile into the argv
passed to `ptyhost`. Browser requests choose `claude`, `codex`, or `grok` and a
working directory; executable, fixed arguments, and environment still come
from server configuration, so request JSON cannot become an arbitrary command.

## Configuration compatibility

The launcher JSON contains `host_binary`, `host_dir`, and either `adapters` or
`profiles`. Existing deployments may keep `schema`. Unknown JSON
members and duplicate environment-map keys are accepted by normal serde mapping.
Adapter/profile IDs use the persisted identifier alphabet and do not need a
version suffix.

The host and CLI executable paths must be usable absolute executable files, and
the host directory must be usable by `ptyhost`. The launcher rechecks the exact
configured executable and host resources before spawning so it does not launch
a file replaced after configuration was loaded. Executable symlinks are resolved
like Python's `shutil.which` result.

Profiles provide fixed `args`, optional legacy `new_args` and `resume_args`,
`env`, and `env_remove`. Configured argument templates remain compatible, but
missing identity templates no longer disable a source:

- Claude new: `--session-id <generated UUID>`; resume: `--resume <sid>`.
- Codex new: no assigned SID; resume: `resume <sid>`.
- Grok new: `--session-id <generated UUID>`; resume: `--resume <sid>`.

When a configured template contains an exact `{session_id}` or `{sid}` argument,
the launcher substitutes it. Otherwise it appends the Python command above.
Fixed profile arguments stay before those identity arguments.

The child inherits the service environment, then applies `env` and
`env_remove`. Session-lineage variables are cleared exactly as in Python's
spawn cleanup: `CLAUDE_CODE_SESSION_ID`, `CODEX_COMPANION_SESSION_ID`,
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
then by original spelling, and returns at most 50 rows (default 24). Python's
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

Synthetic tests cover argv defaults, inherited environment
cleanup, legacy configuration loading, unrestricted canonical working
directories, symlink completion, tab-containing directory names, resource
replacement detection, and receipt identity.
