# Terminal final screen

When a shell (SSH) session's host exits, it saves the terminal's last screen
once, as a reset grid snapshot carrying all retained scrollback history, the
same format a grid attach starts with ([terminal grid](terminal-grid.md)).
An exited SSH session's console shows that final screen read-only, after a
reload or from another page too, until the session is deleted. Nothing records
the session while it runs: there is no byte log, timeline or playback.

Only shell sessions keep one. An agent session's record is its native
transcript, so the launcher starts every non-shell host with `--no-record`
(`Launcher::command`, [lifecycle launcher](lifecycle-launcher.md)) and its
exited console says 实例已退出.

History: until 2026-10-06 hosts recorded every SSH session as segmented
output logs under `records/`, replayed with a timeline. That was removed as
unused. The service deletes such recordings once their host is gone
(`final_screen::remove_legacy_recordings` at startup) and 删除 removes them
with the session; a host that started before the change keeps recording until
it exits and leaves no final screen.

## On-disk layout

Each exited session is one directory under the host's `--dir`:

```text
<dir>/screens/<created_ms>-<host_pid>-<name>/
  meta.json
  snapshot.json
```

`<name>` keeps only `[A-Za-z0-9._-]`; every other character becomes `_`, at
most 64 bytes. An empty result or a name that would start with `.` gets a
leading `_`. The host writes both files into `screens/.<id>.tmp/` (mode 0700)
and renames the directory, so a reader sees either nothing or both files.

`snapshot.json` is one grid `snapshot` object with `reset: true`, `history`
holding every row the host still retained (up to its `--history`, 10000 by
default) and `history_total` equal to its length. `meta.json`:

| Field | Meaning |
| --- | --- |
| `name` | session name at exit |
| `host_pid` | host process id |
| `created_ms`, `ended_ms` | Unix milliseconds at host start and at exit |
| `argv`, `cwd` | the child's argv and working directory (`""` if none) |
| `meta` | the host `run --meta` JSON object |
| `cols`, `rows` | size at exit |
| `exit` | the host's attach exit payload: `code`, `output_complete`, and `reason` when incomplete |

## Host

```text
ptyhost [--dir DIR] run --name N […] [--no-record] -- CMD...
```

The host keeps a final screen by default; `--no-record` turns it off (the name
predates final screens; the launcher probes whether a configured host binary
accepts it and omits it for one that does not). At exit the host first drains
the model of every published byte, then writes the final screen, then sends
the exit frame to attached clients, so a page that observes the exit can read
it at once. A failed write prints one `ptyhost: 最终画面保存失败` line and does
not change the exit. Always pass an explicit `--dir`
([session host](session-host.md)).

## Service and browser

`/api/term/list` adds `final_screen: {id, ended_ms, created_ms}` to a pending
(launch) row when a final screen of its host name exists; the newest wins.
An exited shell row stays listed until 删除 with or without one
(`pending_listed`).

`GET /api/term/final?id=<id>` answers `{exit, ended_ms, cols, rows, snapshot}`,
`snapshot` being the saved grid snapshot as written.
An invalid or unknown id is `404 final_screen_not_found`; a node without the
explicit ptyhost directory answers `501 terminal_disabled`. The Hub page uses
`api/nodes/<node>/api/term/final`. No ownership lease is taken and no host
process is contacted.

The console (`term.js` `showFinalScreen`) opens an exited row's final screen
in the console pane: no claim, no input, no WebSocket, no history paging. A
live SSH console that observes its host exit switches to it in place; if the
row has no final screen yet it reloads the terminal list once, and if none
exists it keeps the content already on the page. The screen keeps the host's
columns and rows, scrolls inside `#xterm`, and shrinks its font to fit a
narrower pane. A status line under the terminal (`#term-final-status`) names
the exit code. Without a final screen (an older host, `--no-record`) the
console says 会话已结束，没有留下最后画面. `term/discard` deletes the session's
final screens and any legacy recordings of its host name.

## Validation

[`tests/terminal_final_screen_browser.py`](../tests/terminal_final_screen_browser.py)
covers live exit on the watching page, the listed row and on-disk layout, and a
fresh page on node and through the Hub. [`tests/lifecycle_browser.py`](../tests/lifecycle_browser.py)
covers a row without a final screen and deletion.
