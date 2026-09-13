# Browser terminal ownership

`terminal::ownership` is a pure, bounded state machine. It has no host discovery,
PTY operations, filesystem writes, WebSocket I/O, authentication middleware, or
background task. Its existence does not enable terminal endpoints or authorize
access to a host.

## API boundary

Create one process-wide `Registry::new(capacity)`; the accepted capacity range is
1 through 4096. Tests can supply `Registry::with_clock(capacity, Arc<dyn Clock>)`.
The clock separates monotonic deadlines from display-only Unix timestamps.

| Operation | Result and responsibility |
| --- | --- |
| `claim(name, page, ip, force)` | Reserve a verified host name, return `ClaimResponse`. Another page receives a conflict unless `force` is true. A same-page reconnect replaces its previous lease. |
| `ClaimResponse::status()` / `into_api_json()` | Produce legacy HTTP status/body. This explicit conversion is the only API that exposes the successful claimant's browser token. Set `Cache-Control: no-store`. |
| `bind(name, page, token)` | Bind one unexpired reservation once; return an opaque `BoundLease` with a newly allocated server connection ID. |
| `BoundLease::revocations()` | Obtain a watch receiver. Check its current value before waiting, and drop borrow guards before I/O or `await`. |
| `is_current(&bound)` | Check the exact name/page/token/connection tuple before accepting a terminal operation. |
| `release(&bound)` | Release only that bound connection. An old transport's cleanup cannot remove its replacement. |
| `release_reservation(name, page, token)` | Cancel only an unbound matching reservation. A token alone cannot release a bound connection. |
| `owner(name)` | Read display-only IP and claim time. No token, page ID, or connection ID is exposed. |
| `expire()` | Sweep abandoned reservations. Ordinary claims and reservation lookups also sweep; a service tick is optional. |

The reservation expires at `now >= deadline`, exactly 15 seconds after a claim.
Binding before that deadline consumes the reservation; a live bound connection
does not expire after 15 seconds. WS code must call `release` on all completion,
attach-failure, and cancellation paths. Dropping `BoundLease` does not release it
automatically; if cleanup is lost, another page can still explicitly force a new
claim. Capacity limits never evict a live lease.

## Identity, secrecy, and validation

Ownership is page ID + a 256-bit OS-random server token + the registry-allocated
connection ID. IP and wall time are display-only. The token's wire representation
is 64 lowercase hex characters, still opaque to the frontend. It is unrelated to
the ptyhost TCP authentication token. Stored tokens and bound credentials have no
`Debug` or generic `Serialize` implementation. Tokens are compared as fixed-size
bytes using `subtle::ConstantTimeEq`; input decoding has no access to the stored
secret. Errors never contain supplied or stored tokens.

Host names must be 1–128 ASCII bytes from `[A-Za-z0-9_.-]`, may not start or end
with `.`, and may not be a Windows reserved device basename (`CON`, `PRN`, `AUX`,
`NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`, including an extension). This is deliberately
a narrower portable subset of the host client's filename validation. Pages must
be 1–128 ASCII bytes from `[A-Za-z0-9_-]`; legacy UUID and hex page IDs are valid.
Nothing is silently truncated, trimmed, case-folded, or interpreted as a path.

The HTTP layer must authenticate the request, enforce origin/loopback policy and
request limits, and verify the requested name belongs to its explicit development
host inventory **before** claiming. Never log token-bearing bodies, WS query
strings, or a converted claim response. This module has no host-membership
knowledge and is not an authorization substitute.

## Replacement and transport integration

A successful replacement installs the new lease while holding the state mutex,
then releases the mutex and signals the former connection. The state machine
never performs I/O or waits for the former WebSocket/host attachment to close.
Same-page replacement has `notify: false`; takeover by a different page has
`notify: true` and includes the new display IP. `Released` signals are silent.
Signal delivery is separate from transport closure, so a claim can already have
been replaced again before its HTTP response arrives; `bind` remains authoritative.

The registry's `is_current` alone is a snapshot, not a transaction with external
host I/O. `TerminalService` therefore adds a separate async gate per host name:
claim publication, attach, binary input, and resize all use the same gate. Attach
and writes recheck the precise lease while holding that gate through their short
I/O deadline. Force waits for an already-started operation to finish or fail; no
old-lease write can start after successful replacement. Gates for different host
names are independent, and their weak-reference index is bounded by admitted
operations/connections, not historical names. No std-mutex or watch borrow guard
is held across I/O or `await`.

Bytes already handed to the OS/host before replacement cannot be recalled; the
gate is not a new atomic protocol inside ptyhost. A failed/timed-out/cancelled
host write is ambiguous and its attachment is dropped without resubmitting input.

All state is in memory. Server restart invalidates every browser lease; it must
not stop the independent host process. Tests use synthetic names, injected clocks,
and local threads/channels, without starting a host, shell, or paid CLI. Linux
compilation does not constitute Windows/macOS runtime validation.

## Instance-bound service leases

`TerminalService::claim_bound(&BoundTarget, page, ip, force)` accepts a caller's
uniquely resolved full native source/SID/UID and explicit host instance. It takes
the per-name IO gate before freshly probing host identity and guard capability,
and publishes a claim only if they still match. A failed fresh probe does not
revoke an existing lease, even when force was requested.

The registry stores the immutable target directly in its bounded lease record,
not in a separate name-indexed map. `prepare_bound(name, page, token, uid,
instance)` atomically consumes the reservation only for its exact pinned UID and
instance. The resulting connection holds that target until cleanup. Reservation
TTL, cancellation, release, and replacement reclaim the lease's target reference;
old cleanup cannot remove a replacement's target.

Raw `prepare` cannot consume an instance-bound lease. Raw `claim` also cannot
downgrade an existing bound lease, even for the same page or with force. A bound
claim may upgrade raw ownership or replace another bound lease according to the
normal page/force rules. Unbound compatibility APIs remain available separately.

A bound prepared connection uses only `HostClient::attach_bound`. The receiving
host's `guarded_v1` validation closes the remaining record-read/connection race;
missing or incorrect guard acknowledgement never yields a writable attachment.
Once connected, input and resize continue on that same guarded stream under the
ownership gate. The bridge never reconnects it by bare name. A successful force
claim prevents the old connection from beginning another write, while already
transmitted bytes remain non-retractable. See [terminal identity](terminal-identity.md).

`cargo test -p sessiondock --test terminal_bound --locked` exercises HTTP and
WebSocket binding with artificial native catalog rows and fake hosts. A missing
acknowledgement is tested as ambiguous: host attach may already have resized after
a valid guard, although the client exposes no writer and sends no browser input.

## Explicit-directory HTTP / WebSocket bridge

No `SESSIONDOCK_PTYHOST_DIR` means no terminal service. A configured directory must
already exist; it is frozen to its canonical location and never populated with
CLI processes by this service. It must be an operator-controlled isolated local
directory, not an untrusted writable share. The server still binds only loopback.

- `POST /api/term/claim` validates the name/page, probes `info` on that exact
  indexed local endpoint, and rejects absent, exited, unreachable or malformed
  hosts before issuing a token. Legacy diagnostic fields are bounded and ignored.
- `GET /api/term/attach` consumes the reservation and upgrades to WebSocket.
  A captured RAII guard releases the exact lease and connection permit if upgrade
  fails, the async task is cancelled, host attach fails, or forwarding ends.
- Browser input/output remains binary raw PTY bytes, including split/invalid
  UTF-8. Text accepts only `{ "t": "resize", "cols": 100, "rows": 40 }`.
  Dimensions are bounded to 1–500 columns and 1–300 rows. Text is never shell input.
- Different-page takeover emits `{ "t": "revoked", "ip": "127.0.0.1" }`
  then close code 4001 (`revoked:<ip>`); same-page replacement is silent 4001
  (`replaced`). Attach/protocol errors use 1011, invalid control 1008, oversized
  browser input 1009, stalled browser output 1013, and Web shutdown 1001.
- PTY output is drained in order before normal EOF/exit closure. An explicit
  finish marker travels behind queued data, so coalesced ack/replay/exit cannot
  discard the final output bytes. Revocation/shutdown still cancel promptly.

Default bounds are 16 browser connections, 8 concurrent control/discovery
operations, 256 reservations, 64 KiB per browser message, 8 MiB per host frame,
and 16 queued output chunks of at most 32 KiB. Host-frame decoding is bounded but
not streaming within a single frame; a replay exceeding the configured 8 MiB
frame limit fails explicitly. Host/control writes and downstream backpressure
have 2-second deadlines; partial host frames have a 3-second deadline. Closing
the browser is best-effort within another 250 ms, after dropping both local host
stream halves. These are payload bounds, not an RSS guarantee.

Only the framed attachment writes binary input/resize; no `send`, `paste`,
`create`, `takeover`, `kill`, or CLI launch API is exposed. Instance-bound
claim/attach require the explicit host directory and a uniquely verified native
identity; this does not enable automatic process discovery or session creation.
`/api/term/list` publishes only fresh, uniquely associated, running, guard-capable
hosts as `sessions`, including full UID/SID/source/instance. `enabled` is true only
when that list is nonempty. `pending` and `sources` remain empty; raw `hosts`
summaries are informational, not authorization. They expose no credentials,
endpoints, argv, or arbitrary metadata. `terminal` and `terminal_transport` are
configured independently of `terminal_create:false` and `outbox:false`.

The legacy console captures one UID/instance tuple for claim, force retry, and
WebSocket attachment. Rust mode never falls back to name prefixes or a different
UID. Saved open layouts also carry the tuple; replacement disposes stale views
and does not automatically restore their layout onto a new instance. Composer
drafts and pending outbox records are not migrated or cleared.

Use `cargo test -p sessiondock --test terminal --locked` for synthetic TCP
peer + HTTP/WS tests. `python3 tests/terminal_browser.py` is an additional free
POSIX acceptance test: after building `sessiondock` and `ptyhost`, it creates
one temporary host running a fixed shell command loop, exercises Chromium and
the existing xterm renderer, verifies resize/takeover, restarts Web without
restarting the PTY, and exits/cleans up only that test shell. It never starts a
model CLI or reads a native home. Real-shell validation has been run on Linux
only; Windows/macOS remain unverified.

`python3 tests/managed_terminal_browser.py` additionally exercises the ordinary
legacy console button and keyboard with a synthetic native UID: two independent
browser pages, confirmed force/revocation, mobile navigation, and Web restart
while preserving the same shell process and unchanged native fixture bytes.

Explicit host exit is ordered after all queued output. An incomplete PTY drain
closes WebSocket with 1011 and a fixed, specific reason; normal exit uses 1000.
A socket EOF without an exit marker uses 1011 and is not evidence of child exit.
The client never forwards arbitrary peer error strings or labels legacy exit
markers without a completeness field as verified complete output.

In Rust mode, a confirmed exit retains the xterm and its final bytes, stops
automatic claims/reconnects for that instance, and removes only the saved open
layout. Drafts/outbox data remain untouched. Since WP-E a complete exit
(`host exited`) closes the pane like Python and returns the page to the
conversation; the explanation goes to the header notice (unless the stop
action already announced its stage) instead of being painted over the CLI's
own farewell text, and the final output stays in the retained (hidden) view.
An incomplete drain (`host output incomplete`) keeps the pane open with the
reason written into the xterm. The console button stays visible and
clickable: gray with the exit explanation when the source has no
resume-capable profile, otherwise usable as "接管会话" (a click starts a
fresh `--resume` through `term/takeover`; the exited instance itself is never
reclaimed). A fresh, uniquely validated different instance clears that
diagnostic; a missing live row does not imply a replacement.
`tests/terminal_exit_browser.py` exercises normal EOF and delayed tail/drain
timeout using a real isolated shell, desktop and mobile, including
hover/focus/click explanations and the absence of automatic reclaims;
`tests/session_stop_browser.py` covers the resumable variant.

Managed terminal discovery has its own Rust-only 3-second refresh when global
`live` is disabled. It waits for the previous observation, skips hidden pages,
and never claims a terminal. A real same-name replacement test verifies stale
exit diagnostics clear but the old saved layout/lease does not transfer: only a
new button click attaches to the newly observed instance with a fresh xterm.
