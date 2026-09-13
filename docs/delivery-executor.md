# Claude reliable send (batch 31)

Batch 31 connects the [pure Claude domain](delivery.md), the
[durable store](delivery-store.md), the [engine](delivery-engine.md) and the
[async service](delivery-service.md) to a real terminal driver, a native
acknowledgment adapter and the four Python send routes, for **Claude main
sessions on managed instances**. Batch 32 reuses the same executor, driver
and routes for Codex main sessions — see
[delivery-codex-executor.md](delivery-codex-executor.md); Grok reliable send
is not enabled. Nothing in this batch fabricates confirmation from screen
text: a receipt becomes confirmed only from the session's own JSONL.

## What it is made of

| File | Role |
|---|---|
| `delivery/driver.rs` | `TerminalDriver` trait + `HostTerminalDriver`: capture screen+cursor, recognize Claude's composer (`inspect`; batch 32 adds `inspect_codex`/`inspect_for`), paste, press keys, acquire/release a server-owned lease over the existing terminal service |
| `delivery/claude_adapter.rs` | Turn one checked read of the session's committed `user` inputs into the Claude Machine's `UserEvidence` (association `VerifiedEnter`) |
| `delivery/executor.rs` | `DeliveryExecutor`: drives engine dispatch batches, per-session serialization, bounded admission, the confirmation/tracking loop; `ManagedResolver` resolves the unique managed instance; batch 32 generalizes it over a `Provider` (Claude / Codex) |
| `sessions::claude_native_inputs` | Checked, restamped read of the current fence and the human `user` inputs committed after a fence (no ad-hoc file reads) |
| `api/delivery.rs` | `POST /api/session/send`, `/draft-status`, `/outbox/retry`, `/outbox/discard` |

## Ownership: a server-held lease in the browser's registry

The executor sends through the **same terminal ownership registry the browser
uses**. For each operation the driver either:

- **borrows the page's own lease** when the request carries it and it names the
  exact instance (the legacy composer, under the `outbox` capability, attaches
  `{page, token, instance_id, launch_id?}` — its open console lease); or
- **claims a server-held lease** (`page = "sessiondock-delivery-executor"`,
  `force = false`) when no page lease is supplied.

Because it is an ordinary claimant, a browser page holding the console gets the
documented ownership error if the server tries to claim, and a second page with
no lease gets it when it sends: `409 terminal_ownership` (the exact conflict
carries the owner IP; a launch-console lease held elsewhere is the binding
mismatch, still `terminal_ownership`). The lease is released after the operation;
a borrowed page lease is never released by the executor. Every host write goes
through `guarded_v1`/launch-guard dispatch on the pinned instance, resolved from
the runtime catalog and lifecycle bindings — never by name, cwd, time or PID.

The target is resolved by `ManagedResolver`: the unique guard-capable managed
host whose **verified native association** names the UID (runtime catalog),
authorized through the lifecycle binding when it originates from a launch
receipt. Duplicate, unmatched, exited or unauthorized instances are
`terminal_unlinked` (409).

## Composer inspection and draft consent

The driver reproduces the Python bridge's composer model on the host **screen
capture**: two rule lines around a `❯` editor row, dim suggestion text vs a real
draft, and cursor position deciding `empty` / `editing` / `unknown` when colour
information is absent. The consent token is the Python fingerprint —
`sha256(cursor_x \0 cursor_y \0 screen)` — returned as `draft_token` and reused
as the domain's frame token.

The host `capture` reply carries `lag`/`dropped` health counters. **The screen
model does not always carry them**: a host that omits them reports `None`, which
is *unknown health, not idle*. When the host **does** expose them, the driver
refuses to treat a lagging capture (`lag > 0`) as a known composer — it returns
`unknown`, so an unread editor is never mistaken for empty, and it aborts a
prepare/Enter whose `dropped` counter moved between capture and write.

Draft flow, matching Python `overwrite_draft`:

1. `draft-status` returns `{draft_state}` plus `{draft_conflict:true, draft_token}`
   while editing.
2. `send`/`retry` with a non-matching or empty `overwrite_draft` against an
   editing composer returns `409 {draft_conflict:true, draft_token}` and
   **persists no receipt** (consent is checked before any durable write).
3. `send`/`retry` with the exact `overwrite_draft` token clears only that
   approved draft (`C-u C-k`), verifies the editor is actually empty, then
   proceeds. A different editing token is a fresh conflict.

## Persist-before-inject, two steps, crash = uncertain

Every terminal mutation follows a durable commit, exactly as the domain
requires (`delivery.md`):

```
persist Queued → DispatchNext → InspectComposer
  → persist PrepareInFlight → paste (verify the exact composer holds the text)
    → persist EnterInFlight → Enter
      → Uncertain (even when the transport call returned)
        → native user record after the boundary → Accepted
```

Paste and Enter are **two separately persisted steps**. A crash between them
restores the receipt as `Uncertain` and is never re-injected. A transport
timeout/EOF after submission (`terminal_input_ambiguous`) is `Uncertain`, never
retried. Preparation requires the driver to independently observe the complete
payload in the composer (whitespace-insensitive) before Enter; if it cannot, the
row stays `Uncertain` and Enter is not authorized. Before Enter the driver
rechecks the frame (screen token, or the composer-rows fingerprint when only
spinners/footers changed) and the health counters.

Serialization: one `tokio::sync::Mutex` per scope (UID) serializes send / draft /
retry / discard and background dispatch for that session; bounded worker
admission (`workers`, default 4) caps concurrent operations across sessions.
Physical terminal write critical sections are already mutually exclusive in the
ownership gate.

## Native acknowledgment: JSONL only, `VerifiedEnter`

Confirmation comes only from the session file, through the SessionStore's
checked readers (`claude_native_inputs`), never an ad-hoc read:

- The fence captured at Enter time is validated exactly like a message
  checkpoint (`source_identity` = the view identity, plus head/anchor). An
  invalid fence (rewrite/truncation/identity change) yields no inputs and never
  a guess.
- A `user` record is accepted when it is a real human input whose record begins
  at or after the confirmation offset, carries a stable record UUID, and whose
  text equals the delivered text after **end-trimming only** (internal spaces
  and newlines significant). It is attributed to this receipt's Enter via
  `Association::VerifiedEnter` — the executor itself held the write lease,
  verified the composer and pressed Enter, so the first matching human input
  after that boundary is this delivery. Records before the fence, without a
  UUID, or already consumed by another receipt never acknowledge.
- Without a qualifying native text-and-time match the row stays uncertain.
  Screen text and assistant activity do not acknowledge it, and the real
  Claude TUI supplies no request-ID echo.

The tracker re-reads from the advancing watch cursor; when a receipt is overdue
(Python `CONFIRM_TIMEOUT`, 8 s) it re-reads from the **fixed confirmation
fence**, rate-limited to once per interval. Automatic tracking stops after a
one-hour window (annotating the receipt once with the domain's timeout issue,
without making it retryable). A dismissed (discarded) receipt is dropped from
automatic tracking so a later identical human input acknowledges the next
receipt, not the tombstone.

## HTTP contract vs Python

All four routes are JSON, same-origin/loopback, `Cache-Control: no-store`, and
return `501 delivery_send_disabled` unless **both** the delivery ledger and the
terminal transport are configured (capability `outbox:true`). Bodies accept the
Python fields; unknown fields (`activity`, `cursor`, `page_id`, diagnostics) are
ignored; the page's own lease is an added optional `lease` object.

### `POST /api/session/send`

Body: `uid`, `name`, `text`, `request_id`, `overwrite_draft`, `media`,
optional `lease`, `_build`. Behaviour mirrors Python `_queue_message` +
`_queue_claude_message`:

- Text writes reproduce the stale-build gate before any terminal access:
  `_build` ≠ served build → `409 {code:"stale_build", reload:true, build}`.
- `request_id`: an empty value mints a UUID; otherwise keep the first 128
  Unicode characters, including whitespace and punctuation, like Python. Retry
  and discard use the returned ID exactly without trimming or truncation. A different
  payload for a known ID → `400 request_conflict` ("重复发送 ID 对应了不同消息").
  A **replay of the same ID/payload is a status lookup**, never a second paste;
  a confirmed row replays as `state:"confirmed"` with no text.
- Success: `200 {ok:true, item, outbox, outbox_version}`. `item.state` is the
  legacy projection (`ambiguous` for an uncertain in-flight row, `attempts:1`).
- Unknown session → `400 session_error`; `name` not the session's unique
  managed instance → `409 terminal_unlinked`; lease held elsewhere →
  `409 terminal_ownership`; draft conflict → `409 {draft_conflict, draft_token}`.
- **Attachments**: the composer uploads files first and embeds their paths in
  the text. `media` remains opaque preview metadata in the outbox and does not
  create a second terminal input.

### `POST /api/session/draft-status`

Body: `uid`, `name`, optional `lease`. Returns `200 {ok:true, draft_state}` plus
`{draft_conflict:true, draft_token}` while editing; a failed capture is
`{draft_state:"unknown"}` (Python `composer_probe`), never an error.

### `POST /api/session/outbox/retry`

Body: `uid`, `id`, `overwrite_draft`, optional `lease`, `_build`. Retry exists
only for the domain's retryable rows — a **local waiter that was never written**
(re-inspection/dispatch). Any attempted row still in the outbox → `409`
("消息已经提交到终端，禁止盲目重发…"); an unknown row, or a confirmed row that
has already left the outbox → `404`. Same stale-build gate as send.

### `POST /api/session/outbox/discard`

Body: `uid`, `id`. Dismisses the row (hides it, cancels an unwritten waiter),
keeps the deduplication tombstone (a later replay of the ID is still a lookup),
never cancels Claude. Unknown/already-gone row → `404` (Python parity).

`GET /api/session/outbox` is unchanged (read-only projection). The front-end
retires its **optimistic** row from the native SSE record independently; the
**server ledger** row is retired by the tracker's confirmation, so the two can
lag by a tick.

## Legacy composer

Under `outbox:true` the existing legacy composer/outbox is enabled unchanged
(epoch/revision handling, stale-build gate, `syncServerOutbox`, the "检查终端"/
"移除"/"重试" controls). Its send / draft-status / retry now attach the page's
console lease (`termSendLease`) when this page holds the console, so composing
from the page that owns the console never conflicts with itself; without a lease
the server claims its own and reports any other page's lease as the ownership
error. A raw text-submit with Enter still routes to the reliable-send composer
(no change to the raw `term/send` input path).

## Validation

- Unit: `cargo test -p sessiondock --lib delivery::driver` (composer model,
  dim suggestions, lag/dropped health, fingerprints, busy footer),
  `delivery::claude_adapter` (end-trim match, byte-order, fence invalidation,
  VerifiedEnter), `delivery::executor` (persist→inject two-step, confirm from
  native record, request-ID replay, draft consent, lease-held ownership,
  swallowed line uncertain + retry refused + discard, restart recovery without
  re-injection, unknown composer, tracking window, FIFO).
- Integration (real router + temporary ptyhost + launcher + ledger, fake Claude
  CLI only): `cargo test -p sessiondock --test delivery_send` — send →
  receipt persisted before injection → native `user` record appears → confirmed;
  request-ID replay; uploaded media path delivery; wrong name / unknown session; console draft
  consent; slow (busy-TUI) late confirmation; swallowed line stays uncertain,
  retry refused, Web restart does not re-inject, discard retires, later resend
  works; and the routes are `501` without the ledger/transport.
- Browser: `python3 tests/send_browser.py` (desktop + 390 px) — compose and send
  through the real legacy composer against the fake CLI, message appears in
  history via SSE, outbox row replaced and server ledger emptied, a second page
  refused with `terminal_ownership` on both composer calls while the console is
  held, and a 390 px send through the server-claimed lease.

The fake Claude CLI (`tests/fake_claude_cli.py`, marked `# run_validation: skip`)
is a private shell/Python script launched through the schema-2 launcher profile
with `--session-id {session_id}`; it renders a Claude-like composer and appends
a synthetic `user` JSONL record for each submitted line, with `--delay`/
`--busy-footer` and `--swallow N` modes. No model binary, native CLI home or
production host is used.

## What stays uncertain or unsupported

- Real-CLI verification against a live `claude` binary
  (`tests/send_claude_real.py`) runs only with `--include-real` or direct invocation, using
  the cheapest configuration (`claude-haiku-4-5-20251001`, `--effort low`,
  exact model ID asserted from the JSONL, temporary `CLAUDE_CONFIG_DIR` reusing
  the login read-only, proxy variables passed through); it skips with a
  printed reason when the binary is absent or a standalone call cannot
  authenticate. Grok is not a send target; the Codex real-CLI suite
  (`tests/send_codex_real.py`, batch 32) skips with the printed reason when
  `codex` is absent, unauthenticated or over its usage limit.
- Grok reliable send: no executor is wired. Codex is wired in batch 32
  ([delivery-codex-executor.md](delivery-codex-executor.md)).
- Uploaded attachment preview metadata is preserved with the receipt.
- Native queue (`enqueue`/`dequeue`/`popAll`) association, `/rename`,
  `/compact`, hook-backed answers, and stop/interrupt are domain-supported but
  not driven by this batch's executor (ordinary prompt delivery only). Screen
  text never confirms; a lagging capture is never treated as idle.
