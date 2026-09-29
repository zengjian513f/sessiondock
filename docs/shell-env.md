# Login-shell environment

The CLIs SessionDock launches need the operator's interactive environment:
API keys, conda/`PATH` additions and the like that only the shell startup files
set. The service itself is started through the login-shell wrapper once, and
every CLI inherits the service's environment. Launch profiles name the CLI
binary directly, without a per-launch wrapper.

A per-launch wrapper (`with-zshrc claude`) costs a full shell startup (~1.5 s)
on every new session and on every `opencode api` call, such as each OpenCode
delete. A wrapper around the service costs that once, at service start.

## Deployment (Linux user unit)

```ini
[Service]
ExecStart=/home/<user>/.local/bin/with-zshrc /srv/sessiondock/bin/sessiondock
```

The wrapper must `exec` its arguments so the service stays the unit's main
process. `KillMode=process` and `Restart=on-failure` stay as they are.
`etc/env` adds:

| Variable | Meaning |
| --- | --- |
| `SESSIONDOCK_SHELL_ENV_COMMAND` | The same wrapper. Enables drift detection and the page restart. |
| `SESSIONDOCK_SHELL_ENV_WATCH` | Optional startup files, `:`-separated (e.g. `~/.zshrc:~/.zshenv`, written as absolute paths). A change triggers an immediate recheck. |

Launch profiles (`etc/launcher.json`) then use the CLI's absolute path as
`executable`, for example `/home/<user>/.local/bin/claude` (symlinks are
resolved before every spawn, so self-updating CLIs keep working). A profile's
`env` must not set `PATH`, or it replaces the inherited one. Keep the explicit
values the CLI needs even when the shell differs, such as proxies. A CLI that
the shell environment cannot find may keep a wrapper as its executable.

The service's own outbound connections (hub and node client) do not read
proxy variables, so they are unaffected by what the shell exports.

## Drift detection

`shell_env::ShellEnv` runs `<command> /usr/bin/env -0` with a fixed minimal
environment (`HOME`, `USER`, `LOGNAME`, `LANG` and a system `PATH`). It runs
once at startup to record the baseline, then again when `GET /api/shell-env`
finds that a watched file's size or modification time changed, or when 10
minutes have passed since the last capture. It compares the variables, ignoring
`_`, `SHLVL`, `PWD` and `OLDPWD`. The startup capture runs twice. A variable
whose value already differs between those two runs is minted fresh by every
shell start (`ATUIN_SESSION`, `STARSHIP_SESSION_KEY`) and is never reported. Only names leave the process; values stay in
memory, and the service's own environment already holds them.

`GET /api/shell-env` returns:

```json
{"configured": true, "stale": true, "changed": ["OPENROUTER_API_KEY", "PATH"],
 "started_at": "…", "checked_at": "…", "error": null}
```

Without `SESSIONDOCK_SHELL_ENV_COMMAND` the answer is `{"configured": false,
"stale": false, "changed": []}`. A capture failure (timeout 30 s, non-zero
exit) sets `error` and keeps the previous result.

`POST /api/shell-env/restart` (`409 shell_env_unconfigured` without the
command) answers `{ok, restarting}`. Then it runs the normal graceful shutdown
(the same path as SIGTERM) and exits with status 75, so systemd starts a fresh
service. The fresh service loads the current startup files and records a new
baseline. Managed CLI hosts survive, as on any restart.

## Page

Every 60 s, and when the page becomes visible, the page asks each online
machine (`api/nodes/<id>/api/shell-env` on the hub, `api/shell-env` locally).
The notice at the bottom is a table with the columns machine, changed
variables, `重启` and `忽略`, one row per machine whose environment drifted.
`忽略` hides that exact set of names for the page. With two or more machines to
restart, `全部重启 (N)` above the table sends every restart at once. A
restarting machine's row reads `正在重启…` in place, and the other rows stay.
It keeps that state even while the hub reports the machine offline or the
request fails, until the machine answers with a new `started_at` (at most
2 minutes). Only the newest of overlapping checks is drawn. A machine
without the endpoint (an older build) or without the configuration shows
nothing.

Regression: `python3 tests/shell_env_browser.py`.
