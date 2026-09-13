# Explicit lifecycle launcher

`lifecycle::launcher` resolves an administrator-configured adapter and launches
one explicit ptyhost binary from a durable `StartAuthority`. It does not accept
HTTP argv/environment, discover CLI installations or user homes, assign a native
SID/UID, declare readiness, retry a spawn, or terminate a host when its owner is
dropped. The lifecycle service owns coordination, guarded readiness, cancellation,
and child observation/reaping.

## Private configuration

`read_config(absolute_path)` reads a single explicit JSON file. It requires a
regular file, Unix mode `0600`, one hardlink, and at most 64 KiB. It walks each
ancestor through no-follow directory handles, opens the leaf nonblocking and
without following symlinks, and verifies directory/file identity plus size/time
stamps around the read. Symlinks, Windows reparse points, unknown fields, duplicate
struct fields, and duplicate environment keys fail closed. It never reads a
default file or discovers a home directory. Parse and filesystem errors returned
by the module contain no configured paths, arguments, environment, or raw parser
input.

The configuration is a private server allowlist, not a browser request:

```json
{
  "host_binary": "/opt/example/bin/ptyhost",
  "host_dir": "/var/lib/example/hosts",
  "cwd_roots": ["/srv/example/work"],
  "adapters": [{
    "id": "example-adapter-v1",
    "source": "codex",
    "executable": "/opt/example/bin/verified-adapter",
    "args": ["fixed-adapter-argument"],
    "env": {"TERM": "xterm-256color"}
  }]
}
```

These are documentation paths and a schematic adapter, not instructions to run
a model or an asserted real CLI invocation contract. Every configuration field,
including `args` and `env`, is explicit; omitted fields do not imply inheritance.
The types deliberately omit Debug and Serialize because their contents are
private administrator data.

`Launcher::new(Config)` verifies:

- Host and adapter paths are normalized absolute UTF-8 paths to existing readable
  regular executables; Unix files must have an executable permission bit.
- The explicit host directory exists, has Unix mode `0700`, and is not a
  filesystem root. The launcher neither creates it nor adopts/removes its data.
- There are 1–16 existing cwd roots, disjoint by normalized path from each other
  and the host directory. No configured cwd root is a filesystem root.
- There are 1–16 unique adapter IDs. IDs use the existing 1–64 ASCII
  letter/digit/underscore/hyphen alphabet and end in `-vN` or `_vN`, where N is
  1–6 decimal digits without a leading zero. `shell-v1` is a valid test ID.
- Each adapter has at most 64 fixed arguments, at most 4096 UTF-8 bytes per
  argument, and no NUL bytes. Empty arguments are allowed and remain separate
  arguments; there is no shell-string joining or quoting step.
- Each environment has at most 64 keys. Keys use standard ASCII environment-name
  syntax and at most 128 bytes; values allow at most 8192 UTF-8 bytes without NUL.
  Configured string/path content together has a 64 KiB budget even when Config is
  constructed directly rather than parsed from a file.

Retained directory/file handles let later validation detect replacement since
construction. `validate_spec(&LaunchSpec)` requires the exact adapter ID and
source, a cwd inside an authorized root (resolved through symlinks first, like Python's `resolve()`, then re-verified component by component), unchanged
root/host-directory identities and unchanged host/selected-adapter file stamps.
The HTTP layer may provide only source, allowlisted adapter ID, and cwd. Executable,
arguments and environment come exclusively from the retained configuration.

The application configuration layer must additionally reject overlap with its
native histories, Web assets, metadata/delivery/lifecycle stores, private config
file and downloadable roots, and require the launcher's host directory to equal
the terminal client's explicitly configured host directory. This module cannot
discover independently configured application paths. It is not a sandbox for
hostile code, malicious same-user filesystem mutation, or bind-mount aliasing.
Adapter identity is an administrator-maintained version contract: changing a
command's meaning requires a new adapter ID rather than reinterpreting a receipt.

## CLI profiles (batch 24, schema 2)

`"schema": 2` additionally allows `profiles`: explicit per-source CLI launch
contracts that keep the same file/permission rules as adapters. `schema`
defaults to 1 (adapters only, existing free-shell configurations unchanged);
adapters and profiles together need at least one entry and at most 16 each,
with IDs unique across both tables and versioned like adapter IDs.

```json
{
  "schema": 2,
  "host_binary": "/opt/example/bin/ptyhost",
  "host_dir": "/var/lib/example/hosts",
  "cwd_roots": ["/srv/example/work"],
  "adapters": [],
  "profiles": [
    {"id": "claude-cli-v1", "source": "claude",
     "executable": "/opt/example/bin/claude",
     "args": ["--settings", "/etc/example/claude-bridge-settings.json"],
     "new_args": ["--session-id", "{session_id}"],
     "resume_args": ["--resume", "{sid}"],
     "env": {"PATH": "/usr/bin:/bin", "HOME": "/home/example"},
     "env_remove": [],
     "cwd_roots": ["/srv/example/work/claude"]},
    {"id": "codex-cli-v1", "source": "codex",
     "executable": "/opt/example/bin/codex",
     "args": ["--enable", "default_mode_request_user_input",
              "-c", "suppress_unstable_features_warning=true"],
     "new_args": [],
     "resume_args": ["resume", "{sid}"],
     "env": {"PATH": "/usr/bin:/bin", "HOME": "/home/example"},
     "cwd_roots": ["/srv/example/work"]}
  ]
}
```

What the configuration decides: the absolute executable (existing regular file,
executable bit, symlinks resolved to the real file, stamp re-verified before every launch), the fixed
`args` prefix, the `new_args`/`resume_args` templates, environment additions
(`env`) and removals from the launcher-built environment (`env_remove`, e.g. the
default `TERM`; must not overlap `env`), and per-profile `cwd_roots` that must
lie inside the global roots (the request cwd is canonicalized through
symlink-free ancestry and must fall inside a profile root).

What stays fixed in code: placeholders are whole arguments only —
`{session_id}` and `{sid}`; embedded forms (`--resume={sid}`), unknown
placeholders and repeats are rejected. Claude `new_args` must contain exactly
one `{session_id}`; Codex and Grok `new_args` must contain none, so a new
session stays a pending identity until operator binding (batch 10).
`resume_args` is either empty (resume unsupported for that source) or contains
exactly one `{sid}`; SIDs are lowercase UUIDs, anything with spaces, slashes or
option prefixes is rejected. The launcher still `env_clear`s and sets a default
`TERM`; profile `env` keys are limited to an allowlist (`HOME`, `PATH`, `TERM`,
`LANG`, `LC_*`, `XDG_*`, `ANTHROPIC_*`, `CLAUDE_*`, `CODEX_*`, `OPENAI_*`,
`GROK_*`, `XAI_*`, `AGENTHUB_TEST_*`, the proxy and certificate variables, and
the Windows system names listed under [Windows](#windows-wp-w); never
`NODE_OPTIONS`), and
`CLAUDE_CODE_SESSION_ID`, `CODEX_COMPANION_SESSION_ID`, `GROK_SESSION_ID`,
`CODEX_THREAD_ID`, `CODEX_SESSION_ID`, `CLAUDE_PID`, `TMUX`, `AGENTHUB_SESSION`
are always refused (the spawn-lineage keys would make a managed instance look
spawned by the Web process). The Python wrapper
(`/bin/sh -c 'env -u …'` inside a login shell) is intentionally not reproduced:
argv is handed to ptyhost `--` as an array with no shell joining, the `env -u`
semantics come from `env_clear` + the denylist + ptyhost's own `STRIP_ENV`, and
login-shell `PATH`/`HOME` are configured explicitly.

### Question cards and approvals: the arguments the profiles must carry

The example above already shows them. Python launches Claude with
`--settings <claude-bridge-settings.json>` so that its `AskUserQuestion`
dialog reaches the conversation page as a live card, and Codex with
`--enable default_mode_request_user_input -c suppress_unstable_features_warning=true`
so that its questions are recorded as `request_user_input` and the unstable-
feature banner does not cover the composer ([delivery.md](delivery.md), WP-G).
For the Rust service:

```sh
# once per deployment, with the service environment (SESSIONDOCK_STATE_DIR set):
/srv/example/bin/sessiondock --write-bridge-settings /srv/example/etc/claude-bridge-settings.json
```

writes a 0600 hooks-only settings file whose five hooks run
`/srv/example/bin/sessiondock claude-hook --state-dir /srv/example/state`
(exec form, no shell, no Python). The hook binary path is the running binary's
own absolute path and the state directory is the configured one, so the file is
regenerated when either moves; the CLI's cleared environment needs nothing
else. Every Claude profile — the interactive one and the bug-report worker —
then carries `"args": ["--settings", "/srv/example/etc/claude-bridge-settings.json", …]`,
and every Codex profile `"args": ["--enable", "default_mode_request_user_input",
"-c", "suppress_unstable_features_warning=true", …]` (before the model flags
of a worker profile). The cards are written under
`<SESSIONDOCK_STATE_DIR>/claude-prompts/` ([environment.md](environment.md)).

### Login-shell wrapper (batch 44 WP-F)

Python starts its host through `~/.local/bin/with-zshrc` (`agenthub/term_host.py`
`_env_wrapper`), so every CLI inherits the interactive zsh environment: the
conda environment's `python3`, `~/.local/lib/npm-global/bin`, `~/.grok/bin`,
the API keys exported by `.zshrc` and the proxy variables from `.zshenv`. The
Rust launcher deliberately `env_clear`s and never reads a shell rc, so with a
bare CLI executable a managed Claude runs `/usr/bin/python3` instead of the
conda one. The launcher needs no change to reproduce Python's environment: the
wrapper *is* the profile executable and the CLI becomes its first argument.

```json
{"id": "claude-cli-v1", "source": "claude",
 "executable": "/home/example/.local/bin/with-zshrc",
 "args": ["claude", "--settings", "/etc/example/claude-bridge-settings.json"],
 "new_args": ["--session-id", "{session_id}"],
 "resume_args": ["--resume", "{sid}"],
 "env": {"PATH": "/home/example/.local/bin:/usr/local/bin:/usr/bin:/bin",
         "HOME": "/home/example", "TERM": "xterm-256color", "LANG": "en_US.UTF-8"},
 "cwd_roots": ["/srv/example/work"]}
```

The wrapper (`#!/usr/bin/env zsh`; `source "${ZDOTDIR:-$HOME}/.zshrc"`; `exec "$@"`)
is an ordinary regular file with an executable bit, so every launcher rule
still applies to it: absolute path, no symlink, stamp re-verified before each
launch, `env_clear` plus the explicit `env`, the identity denylist and
ptyhost's own `STRIP_ENV`. What moves: the CLI binary is now resolved by the
login shell's `PATH` from `args[0]` (`claude`, `codex`, `grok` — exactly what
Python's `shutil.which(source)` resolves), so `~/.local/bin/claude` following
a CLI upgrade needs no launcher change, and `exec` replaces the zsh process
with the CLI, so the host's direct child is the CLI itself (process-tree
liveness, `/proc` fd evidence for autobinding and the guarded stop are
unchanged; a 2 s startup cost comes from the rc files). `HOME` in `env` must
be the real home so that the rc files are found; the explicit `PATH`/proxy
entries stay as the floor the wrapper starts from. The same shape applies to
the bug-report worker profiles: `bug_report_policy` scans the arguments after
the executable for the `--model`/`--effort` (Claude), `-m`/`-c
model_reasoning_effort` (Codex) and `-m`/`--reasoning-effort` (Grok) pairs,
so `["claude", "--settings", …, "--model", "claude-haiku-4-5-20251001",
"--effort", "low"]` still satisfies it (`launcher_tests`
`bug_report_policy_accepts_a_login_shell_wrapper_argv`).

A side effect worth the change on its own: Grok is installed as
`~/.grok/downloads/grok-<version>-linux-x86_64` with a `~/.grok/bin/grok`
symlink, and the launcher's no-symlink rule forced the download name into
argv0, which `runtime::procscan::is_cli` (Python `_is_cli`) does not accept —
so a Rust-started Grok was missing from `/api/live` `tmux_uids`/`started_at`.
Through the wrapper `exec grok` starts it as argv0 `grok`, and Claude's argv0
is `claude` instead of its version directory name.

The wrapper is trusted user configuration like the CLI installation itself;
the launcher does not inspect what the rc files do. On a machine without such
a wrapper, keep the bare executables and put the needed directories in the
profile `PATH`.

`Launcher::argv()` and `metadata()` build the command line and the immutable
`--meta` for `fixed | new_pending | new_assigned | resume{sid,uid}` launches;
`complete_directories()` lists child directories strictly inside the global
cwd roots, never following or listing symlinks, at most 50 entries.

## Windows (WP-W)

The same launcher runs on Windows; nothing is discovered there either, so the
profile has to say what a Windows shell would have provided:

- **Executables are real files, never links.** `winget` portable packages put
  the binary under `%LOCALAPPDATA%\Microsoft\WinGet\Packages\…` and only a
  symbolic link on `PATH` (`…\WinGet\Links\codex.exe`, `grok.exe`). The
  launcher's no-symlink/no-reparse-point rule rejects the link, and a service
  started from OpenSSH as an administrator holds an elevated token that Windows
  does not let traverse a reparse point inside a user profile at all
  (`ERROR_UNTRUSTED_MOUNT_POINT`, 448 — `stat` and `CreateProcess` both fail).
  Write the target: `…\WinGet\Packages\OpenAI.Codex_…\codex-x86_64-pc-windows-msvc.exe`,
  `…\WinGet\Packages\xAI.GrokBuild_…\grok.exe`. Claude's native install
  (`%USERPROFILE%\.local\bin\claude.exe`) is a regular file. Upgrading a
  winget package changes the target name; the profile is then updated by hand,
  as the version contract intends.
- **The environment is cleared, so the system variables go in `env`.** Without
  `SystemRoot` the loader, Winsock and Node's DNS fail with misleading errors,
  and the CLIs read `USERPROFILE`/`APPDATA`/`LOCALAPPDATA`/`TEMP` for their
  own state. The allowlist therefore accepts, spelled as Windows sets them:
  `SystemRoot`, `SYSTEMROOT`, `windir`, `SystemDrive`, `USERPROFILE`,
  `APPDATA`, `LOCALAPPDATA`, `TEMP`, `TMP`, `COMSPEC`, `PATHEXT`, `HOMEDRIVE`,
  `HOMEPATH`, `USERNAME`, `PROGRAMDATA`/`ProgramData`, `ProgramFiles`,
  `ProgramFiles(x86)` (the one name that is not a plain identifier), and
  `ProgramW6432`. Windows variable names are case-insensitive and `Command`
  folds duplicates, so list each once (`SystemRoot` *or* `SYSTEMROOT`). `HOME`
  is still needed: the CLIs' Unix-style code paths read it. `PATH` must name
  the directories the CLI spawns tools from (`C:\WINDOWS\system32` for
  `cmd.exe`, Git, Node). Paths are written with doubled backslashes in JSON.
- **No mode bits.** The `0600`/`0700` checks are Unix-only; Windows relies on
  the profile directory's ACL. Symlink/reparse-point ancestry, the single
  hard link, the read-only bit and the identity/stamp re-verification are
  checked on both.
- **Job objects.** A console window, a scheduled task and most service
  wrappers put the service into a job without `JOB_OBJECT_LIMIT_BREAKAWAY_OK`;
  the first spawn is then refused and the retry keeps the host inside the job,
  so closing that console kills every session. Start the service detached
  (from OpenSSH, whose job allows breakaway, with the same three flags) when
  sessions must outlive it; see the deployment notes.
- **Canonical cwd spelling.** `LaunchSpec` canonicalizes the requested cwd;
  Windows answers `\\?\D:\work\x`, which would match no `D:\…` root and
  would reach the CLI verbatim (Claude names its project directory after
  the cwd). The verbatim disk prefix is folded back to `D:\work\x`
  (`lifecycle::model::plain_canonical`); UNC and device prefixes are left
  alone and match no root. Directory completion (`complete_directories`)
  still joins with `/` and is therefore of no use with backslash roots on
  Windows; the page accepts a typed absolute path regardless.
- **Codex and Grok stay pending until the operator binds them.** The
  process-evidence autobind of a `new_pending` receipt needs the Linux
  `/proc` scan; on Windows the page's binding dialog (`/api/term/bind`,
  `operator_confirmed: true`) associates the native session that appeared
  after the first prompt. Claude carries its `--session-id` and associates
  by declared SID as everywhere else.

```json
{
  "schema": 2,
  "host_binary": "C:\\Users\\example\\sessiondock\\bin\\ptyhost.exe",
  "host_dir": "C:\\Users\\example\\sessiondock\\host",
  "cwd_roots": ["D:\\work"],
  "adapters": [],
  "profiles": [
    {"id": "claude-cli-v1", "source": "claude",
     "executable": "C:\\Users\\example\\.local\\bin\\claude.exe",
     "args": ["--settings", "C:\\Users\\example\\sessiondock\\etc\\claude-bridge-settings.json"],
     "new_args": ["--session-id", "{session_id}"],
     "resume_args": ["--resume", "{sid}"],
     "env": {"PATH": "C:\\Users\\example\\.local\\bin;C:\\Program Files\\nodejs;C:\\Program Files\\Git\\cmd;C:\\WINDOWS\\system32;C:\\WINDOWS",
             "HOME": "C:\\Users\\example", "USERPROFILE": "C:\\Users\\example",
             "SystemRoot": "C:\\WINDOWS", "SystemDrive": "C:", "COMSPEC": "C:\\WINDOWS\\system32\\cmd.exe",
             "PATHEXT": ".COM;.EXE;.BAT;.CMD", "APPDATA": "C:\\Users\\example\\AppData\\Roaming",
             "LOCALAPPDATA": "C:\\Users\\example\\AppData\\Local",
             "TEMP": "C:\\Users\\example\\AppData\\Local\\Temp", "TMP": "C:\\Users\\example\\AppData\\Local\\Temp",
             "USERNAME": "example", "HOMEDRIVE": "C:", "HOMEPATH": "\\Users\\example",
             "ProgramData": "C:\\ProgramData", "ProgramFiles": "C:\\Program Files",
             "TERM": "xterm-256color", "LANG": "en_US.UTF-8"},
     "cwd_roots": ["D:\\work"]}
  ]
}
```

The host itself is the imported ConPTY ptyhost (loopback TCP endpoint and
token in the `.json` record, [session-host.md](session-host.md)); it inherits
this environment and passes it to the CLI after its own `STRIP_ENV`.

What the first Windows version does not do: the `/proc` scan
(`SESSIONDOCK_PROC_SCAN`, `capabilities.live`, external-CLI liveness, process
evidence for autobinding) is Linux-only, so a session that was started by hand
in another window is not seen as running, and taking it over from the page
starts a second instance — exactly Python's documented limitation without
psutil. Managed instances are verified through
[`runtime::process`](processes.md#process-identity-and-the-three-run-states-batch-22)
(`process_identity: "windows_process_times"`), so run state, stop and delete
confirmation work for everything this service started.

## Spawn and ownership API

```text
Launcher::new(Config)                  -> Result<Launcher, Error>
Launcher::validate_spec(&LaunchSpec)    -> Result<(), Error>
Launcher::host_dir()                   -> &Path
Launcher::launch(StartAuthority)       -> Result<Started, LaunchFailure>

Started       { authority: StartAuthority, child: std::process::Child }
LaunchFailure { authority: StartAuthority, error: Error }
```

Launch validates the supplied Starting authority's specification again. It also
rejects an already occupied `.json`, `.sock`, or `.log` endpoint for the generated
host name. It never removes a colliding endpoint or retries another name.

The command uses the configured host binary, explicit `--dir`, explicit `--cwd`,
the generated host name, and immutable metadata containing only the receipt's
source, launch ID and instance ID. The fixed adapter executable/arguments follow
`--`. The parent command's cwd is the same authorized cwd. Its environment is
cleared, then populated only with default `TERM=xterm-256color` and the adapter's
explicit environment (an explicit TERM value overrides the default). stdin,
stdout and stderr are null, so there is no undrained output pipe or inherited
terminal handle. No HOME or PATH is searched or implicitly inherited.

The command line, environment, cwd and null stdio are assembled once
(`Launcher::command`); only the detachment differs per platform
(`spawn_detached`). On Unix the host is put into a new process group with
`process_group(0)`. This is not cgroup escape, a new service-manager unit, or
proof that a systemd stop policy will preserve the host. On Windows the host is
created with `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP |
CREATE_BREAKAWAY_FROM_JOB` (Python `term_host._spawn`); when the service itself
sits in a job object that forbids breakaway, `CreateProcess` refuses with
access denied and the spawn is retried once without the breakaway flag, so the
host starts inside the job and shares the service's fate (see
[Windows](#windows-wp-w)). Any other target returns `SpawnFailed(Unsupported)`
while preserving the authority.

`launch` is synchronous and performs at most one `Command::spawn`. The service
must run it as retained blocking work after durable Starting, independently of
HTTP cancellation. On failure it receives the original authority for a typed
store transition; it must not use that returned authority to retry. On success
there is no subsequent fallible path that drops the Child and returns a false
no-spawn result. The service retains/reaps that exact Child handle and the
authority until it has verified readiness through launch-guarded Info or recorded
an explicit uncertain outcome. Spawn success and metadata-file appearance are
not readiness proofs.

Dropping Launcher or Started does not explicitly kill the host; a plain Child
also does not reap itself. Child reaping is a required coordinator responsibility.
The launcher exposes no PID-based kill, directory scan, native-session binding,
or cleanup API. A service restart cannot reconstruct an owned Child handle from
a PID; it reconciles the durable launch and instance through the separate host
protocol.

Checks are performed immediately before spawning, but std's path-based execution
still has a filesystem race after validation, including the adapter path later
opened by ptyhost. Retained handles detect earlier changes; they are not an
`fexecve`-style executable capability or a guarantee against a malicious same-user
rename race. Explicit allowlisted installations and runtime directories must
remain trusted. No exactly-once external process guarantee is claimed.

## Validation

Ordinary `cargo test -p sessiondock --locked lifecycle::launcher::` uses only
synthetic private files and directories. It checks allowlist/source/cwd rejection,
limits, versioned IDs, symlink ancestry, permissions, duplicate/unknown JSON,
hardlinks, file/root replacement, deserialized cwd validation, and returning the
same authority on endpoint collision without spawning.

The separate ignored free-shell integration requires both
`SESSIONDOCK_TEST_PTYHOST_BINARY` and `AGENTHUB_TEST_FREE_SHELL_BINARY` to be explicit
absolute no-symlink executable paths, then runs the test with `--ignored`. It has
no default binary lookup. Use only a built development ptyhost and a free POSIX
shell. It validates actual launch metadata/guard, configured cwd, cleared HOME,
explicit environment and TERM, preserving the host when Launcher is dropped,
normal guarded shell exit, and reaping the held test Child. No paid CLI, native
history, production host, or recovered PID is involved.
