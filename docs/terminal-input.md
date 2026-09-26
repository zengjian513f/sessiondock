# Raw terminal input over HTTP

`POST /api/term/send` and `POST /api/term/scroll` give the legacy console the
two HTTP input paths it used, under exactly the
same authority as the WebSocket input path. This is raw keystroke delivery:
success means the host acknowledged the write, never that the CLI processed
it. The reliable-send composer, outbox and native confirmation stay
unavailable (`outbox:false`), and no send ledger is involved.

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
live question cards send ([delivery.md](delivery.md)). Exact host names,
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

## `POST /api/term/scroll`

`{name, up?, lines? (default 3), cancel?}` → `200 {pos:0,
scrollback:"browser"}`; 404 `terminal_missing` without a record, 400 on bad
shape, 501 when the transport is off. `scroll()` returns 0 and
`leave_copy_mode()` is a no-op because the browser's
xterm scrolls itself; the host protocol has no view/scroll state (only
`capture` snapshots), so the route performs no host I/O and never writes to
the PTY. For `ptyhost` rows the console leaves wheel events to the grid/xterm
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

`cargo test -p sessiondock --test terminal_input --locked` (temporary
ptyhost running a private `/bin/sh`: text + Enter echoed through capture,
refusal without lease, after revoke and after exit, size limits and input bursts,
the lease-less page written through its pinned instance even with a PTY holder; skips when ptyhost is not built) and
`python3 tests/terminal_scrollback_browser.py` (real wheel up/down over PTY history
in grid and xterm, stable history position, subsequent live input, no HTTP scroll);
`python3 tests/terminal_input_browser.py` (desktop local wheel and WebSocket input,
390 px key bar `Tab`/`Up` over HTTP, exact lease body, no
claim/input after exit, composer hidden, fixture bytes unchanged);
`python3 tests/send_browser.py` (conversation SEND while another page holds the PTY lease); `python3 tests/grok_raw_send_browser.py`
(Grok composer text from a 390 px page without a console: paste + Enter with
an empty token and the pane identity while another page holds the console, no claim, no reliable-send call, the
shell's reply visible once the console opens).
Out of scope: reliable send, Escape's activity side effects, `text`+`enter`
submit semantics, tmux copy-mode.
