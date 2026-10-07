# Raw terminal input over HTTP

`POST /api/term/send` and `POST /api/term/scroll` give the legacy console the
two HTTP input paths it used, under exactly the
same authority as the WebSocket input path. This is raw keystroke delivery:
success means the host acknowledged the write, never that the CLI processed
it. No send ledger or native confirmation is involved; the composer's
reliable SEND is a separate path ([conversation](conversation.md)).

## Authority

Input requires the caller's valid ownership lease for the exact instance —
the reservation token issued by claim or the bound WebSocket lease — and the
same identity expectation the WS path pins (`uid` + `instance_id` for native
`guarded_v1` targets, `record_id` + `launch_id` + `instance_id` for launch
instances, raw name for legacy hosts). `Registry::authorize_input` checks the
current lease without consuming it: no lease → 403 `terminal_ownership`,
revoked/replaced token or binding mismatch → 409, host record gone or exited
while a lease is held → 410 `terminal_exited`. Raw (unbound) targets are
re-probed like raw attach: a launch identity → 409, exited → 410.

A page that holds no lease for the terminal — the conversation view with the
console closed or open on another device — sends an **empty `token`**
with the pinned identity. The route
then resolves the instance exactly as a claim would (runtime catalog and
lifecycle authorization for native targets, the lifecycle receipt for launch
targets). `TerminalService::send_input_unleased` rechecks that pinned instance
and serializes the write under the per-name gate, independently of any browser
PTY lease. Nothing is reserved, replaced or minted; the existing terminal stays
connected. Without an identity there is no name-only write
(409 `terminal_binding_unavailable`).

The conversation view has no takeover action. CHECK and SEND use the same
instance-guarded path without a browser lease. Only explicitly opening PTY mode
asks to replace an existing holder. CLI readiness checks and draft preservation
still apply.

## `POST /api/term/send`

Body (`deny_unknown_fields`): `name`, `page`, `token`, the identity fields
above, optional `agent` (accepted, ignored, ≤ 256), and exactly one of
`data` (UTF-8 string sent as-is, no bracketed paste, no implicit Enter),
`paste` (the host's bracketed-paste path, no implicit Enter) or `keys` (array
of named keys). `data` and `paste` reproduce the legacy stale-build gate
(`_build` ≠ served build → 409 `{code:"stale_build", reload:true, build}`);
keys are not gated. Success:
`200 {ok:true, bytes, acknowledged:true, processed:"unknown"}` with
`Cache-Control: no-store`.

Named keys map to the host's key names and byte sequences: `enter`→`\r`,
`escape`→`\x1b`, `tab`, `backspace`→`\x7f`, `delete`, `insert`, `space`,
`home`, `end`, `pageup`, `pagedown`, `up`/`down`/`left`/`right` (CSI; the host
translates under DECCKM), `f1`–`f12`, and `ctrl-<a-z>` / `C-x` / `^x`
(letter & 0x1f), plus one single ASCII letter or digit (`1`–`9`, `y`, `p`)
typed literally — the Codex question menu and command approval answers the
live question cards send ([below](#live-questions-and-approvals)). Exact host names,
lower-case aliases are normalized. Every other nonempty key name up to the
host's 256-byte per-key ceiling is typed literally.

Limits: 1 MiB decoded bytes per request (ptyhost's own send/paste ceiling),
≤ 256 keys (the host's guarded-operation ceiling), 4 MiB host
control line including JSON escaping. There is no per-second input count limit.
Requests serialize on the per-name gate and use the 10 s host-operation
timeout. Codes: 501
`terminal_disabled`, 400, 413 `terminal_input_too_large`, 403/409/410 as
above, 504 `terminal_input_ambiguous` when the host did
not acknowledge — the write may or may not have happened and is never
retried automatically.

## Console file paste

A CLI reads its clipboard on the node (`xclip`/`wl-paste`, `osascript`,
`Get-Clipboard`), so an image pasted into the browser console never reaches
it and the CLI's own `[Image #N]` path cannot work through SessionDock. The
console offers the drag-a-file equivalent instead, behind the browser
preference `sessiondock.consolePasteFiles` (设置 › 功能 › 控制台粘贴文件, off
by default): `term.js` captures a `paste` event carrying files ahead of the
grid paste handler, writes each file through the raw attachment route
(`POST /api/session/attachment?uid=…&name=…`, one `sessiondock_attachments/<batch>/`
directory per paste, `id` reused for the second file on) and then sends the
relative paths as one bracketed `{paste}` over `/api/term/send` under the
console lease, followed by a space and no Enter: `./sessiondock_attachments/3/shot.png `
(`.\…` on Windows nodes, spaces escaped). The CLI sees a typed path exactly
as a dropped file and reads it on submit. Text pastes never enter this path.
DELTA (2026-10-07): with the switch off a file paste is ignored and a four-second
inline notice inside its console says where to enable it. Upload progress also
stays inside that console and clears when the upload finishes; it never floats
over the page. Every paste surface (console, composer, report form) asks once
before staging more than five files or more than 50 MB in one paste; a
dismissed confirm stages nothing. Failures stay browser alerts; nothing is
retried.

## `POST /api/term/scroll`

`{name, up?, lines? (default 3), cancel?}` → `200 {pos:0,
scrollback:"browser"}`; 404 `terminal_missing` without a record, 400 on bad
shape, 501 when the transport is off. `scroll()` returns 0 and
`leave_copy_mode()` is a no-op because the browser's
grid console scrolls itself; the host protocol has no view/scroll state (only
`capture` snapshots), so the route performs no host I/O and never writes to
the PTY. For `ptyhost` rows the console leaves wheel events to the grid
renderer: ordinary shell output scrolls locally, while mouse-reporting or
alternate-screen applications retain their terminal input behavior. Only old
tmux rows use the legacy HTTP scroll path.

## Legacy

Under `terminal_input: true` (set when the terminal transport is configured)
`term.js` attaches the lease (`name/page/token` plus the identity fields) to
the HTTP paths it already used: `onData` while a scroll request is pending
sends `{data}`, and `sendToSession(null, keys)` — the mobile key bar, the
composer's Esc, the question cards — sends `{keys}`. When this page holds no
lease (`termInputBody` finds no `inputLease`), the body carries an empty token
and the pane row's identity (`termRowBinding`), and the server decides as
above. Text on the raw path — a pending console before its first native
record, or a source without reliable send such as Grok — is
a bracketed `{paste}`, then `{keys:["Enter"]}` 600 ms later so
a paste-burst marker cannot swallow it. A Claude/Codex text submit is the
reliable-send composer instead. Pages without the capability keep the original bodies.
After a host exit the key bar silently does nothing once the instance leaves
the list.

## Validation

`python3 tests/terminal_scrollback_browser.py` (real wheel up/down over PTY history
in the grid console, stable history position, subsequent live input, no HTTP scroll);
`python3 tests/terminal_input_browser.py` (desktop local wheel and WebSocket input,
console file paste off (hint, no upload) and on from the settings switch
(clipboard image and a two-file paste published per batch and typed as
paths, the six-file and 51 MB confirms dismissed then accepted),
390 px key bar `Tab`/`Up` over HTTP, exact lease body, no
claim/input after exit, composer hidden, fixture bytes unchanged);
`python3 tests/send_browser.py` (conversation SEND while another page holds the PTY lease).
Out of scope: reliable send, Escape's activity side effects, `text`+`enter`
submit semantics, tmux copy-mode.

## Live questions and approvals

Claude question cards come from `sessiondock claude-hook`; Codex approvals come
from the managed TUI screen. Main-session message and watch responses expose
them as `prompt`. A native answer clears the card. Only the main thread's
`AskUserQuestion` records a card: Claude denies the tool to every agent, so a
payload carrying `agent_id` (a post-turn fork such as prompt suggestion) never
opens a dialog and is ignored.

The page answers through `/api/term/send` under its console lease. The bridge
never writes to the CLI, takes a lease or acknowledges delivery. Hook files and
generated settings are private and atomic. Codex prompt recognition must match
the frozen bridge. See [lifecycle-launcher.md](lifecycle-launcher.md).

Validate with `tests/question_browser.py`. Run the real
`prompt_claude_real.py` browser suite only under the real-CLI policy in
`AGENTS.md`.
