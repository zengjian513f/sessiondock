# Raw terminal input over HTTP (batch 25)

`POST /api/term/send` and `POST /api/term/scroll` give the legacy console the
two HTTP input paths it used against the Python backend, under exactly the
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

## `POST /api/term/send`

Body (`deny_unknown_fields`): `name`, `page`, `token`, the identity fields
above, optional `agent` (accepted, ignored, ≤ 256), and exactly one of
`data` (UTF-8 string sent as-is, no bracketed paste, no implicit Enter) or
`keys` (array of named keys). `data` reproduces the legacy stale-build gate
(`_build` ≠ served build → 409 `{code:"stale_build", reload:true, build}`);
keys are not gated, as in Python. Success:
`200 {ok:true, bytes, acknowledged:true, processed:"unknown"}` with
`Cache-Control: no-store`.

Named keys map to the host's key names and byte sequences: `enter`→`\r`,
`escape`→`\x1b`, `tab`, `backspace`→`\x7f`, `delete`, `insert`, `space`,
`home`, `end`, `pageup`, `pagedown`, `up`/`down`/`left`/`right` (CSI; the host
translates under DECCKM), `f1`–`f12`, and `ctrl-<a-z>` / `C-x` / `^x`
(letter & 0x1f), plus one single ASCII letter or digit (`1`–`9`, `y`, `p`)
typed literally — the Codex question menu and command approval answers the
live question cards send ([delivery.md](delivery.md), WP-G). Exact host names,
lower-case aliases are normalized. Every other nonempty key name up to the
host's 256-byte per-key ceiling is typed literally, matching Python ptyhost.

Limits: 1 MiB decoded bytes per request (ptyhost's own send/paste ceiling),
≤ 256 keys (the host's guarded-operation ceiling), 4 MiB host
control line including JSON escaping. There is no per-second input count limit.
Requests serialize on the per-name gate and use Python's 10 s host-operation
timeout. Codes: 501
`terminal_disabled`, 400, 413 `terminal_input_too_large`, 403/409/410 as
above, 504 `terminal_input_ambiguous` when the host did
not acknowledge — the write may or may not have happened and is never
retried automatically.

## `POST /api/term/scroll`

`{name, up?, lines? (default 3), cancel?}` → `200 {pos:0,
scrollback:"browser"}`; 404 `terminal_missing` without a record, 400 on bad
shape, 501 when the transport is off. The Python ptyhost backend's `scroll()`
already returned 0 and `leave_copy_mode()` was a no-op because the browser's
xterm scrolls itself; the host protocol has no view/scroll state (only
`capture` snapshots), so the route performs no host I/O and never writes to
the PTY. The legacy wheel handler keeps its existing behaviour for `ptyhost`
rows.

## Legacy

Under `terminal_input: true` (set when the terminal transport is configured)
`term.js` attaches the lease (`name/page/token` plus the identity fields) to
the two HTTP paths it already used: `onData` while a scroll request is
pending sends `{data}`, and the mobile key bar's `sendToSession(null, keys)`
sends `{keys}`. Python-served pages keep the original bodies. A text submit
with Enter in Rust mode explains that reliable send is not available instead
of posting. After a host exit the key bar silently does nothing once the
instance leaves the list, matching Python.

## Validation

`cargo test -p sessiondock --test terminal_input --locked` (isolated
ptyhost running a private `/bin/sh`: text + Enter echoed through capture,
refusal without lease, after revoke and after exit, size limits and input bursts;
skips when ptyhost is not built) plus five input unit tests, and
`python3 tests/terminal_input_browser.py` (desktop HTTP `data` while a scroll
is pending, 390 px key bar `Tab`/`Up` over HTTP, exact lease body, no
claim/input after exit, composer hidden, fixture bytes unchanged).
Out of scope: reliable send, Escape's activity side effects, `text`+`enter`
submit semantics, tmux copy-mode.
