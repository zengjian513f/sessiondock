# Browser terminal ownership

`terminal::ownership` tracks in-memory browser reservations and bound leases.
Host discovery, PTY transport and native identity checks live in the terminal
service and ptyhost client.

## API boundary

`Registry::new()` creates the process-wide registry. It has no application lease,
connection, operation or historical-name quota. `claim(name, page, ip, force)`
creates a 15-second reservation. `bind` consumes it once and returns a
`BoundLease`; a bound lease remains live until release or replacement.

`is_current` checks the exact lease, `release` cannot remove a newer replacement,
and `owner` exposes only display IP/time. Restarting the Web service drops browser
leases without stopping the independent ptyhost process.

## Identity, secrecy, and validation

Ownership combines page ID, a random server token and a server connection ID.
The browser token is distinct from the ptyhost credential, is compared in
constant time, and is never logged. Pages accept Python's nonempty Unicode value
up to 128 characters. Host names retain the real ptyhost filename/protocol rules.

The HTTP layer verifies that a requested native UID and instance exactly match a
freshly observed target. It does not impose a separate ASCII or byte-length policy
on those identifiers.

## Replacement and transport integration

A force claim publishes the new lease and signals the previous page. Terminal
claim, attach, input and resize share a per-host asynchronous gate, so a stale
lease cannot begin another write after replacement. Work already handed to the
host cannot be recalled. The gate waits for an operation already in progress;
there is no admission timeout or Busy result invented by the Web layer.

## Instance-bound service leases

`claim_bound` pins the complete observed native source/SID/UID and host instance.
Attach rechecks that identity and uses `guarded_v1`; it never reconnects by a
name prefix or substitutes a different session. Launch-bound reservations use
the corresponding record, launch and instance identity. Raw compatibility claims
remain distinct from these bound forms.

## Explicit-directory HTTP / WebSocket bridge

A configured `SESSIONDOCK_PTYHOST_DIR` enables the terminal service. Claims probe
the exact host; attach upgrades to WebSocket and releases the exact lease on all
exit paths.

Browser binary messages are raw PTY input. A resize JSON message is interpreted
when it has valid Python dimensions (each 1–1000 and product at most 250,000);
other text is forwarded literally as terminal input, matching Python. Unknown key
names are forwarded literally within the ptyhost wire bounds of at most 256 keys
and 256 bytes per key. Send/paste remains bounded by the host's 1 MiB protocol
frame. Browser WebSocket frames retain the 8 MiB per-frame protocol boundary;
fragmented messages have no separate aggregate application cap.

PTY output is forwarded in order and backpressure waits for the browser. There is
no fixed queued-output count and no two-second backpressure disconnect policy.
Partial ptyhost frame reads have no deadline; ordinary control operations use the
same 10-second timeout as Python. Normal EOF preserves final output, while
revocation and shutdown cancel the bridge promptly.

The browser abandons an attach that remains in WebSocket `CONNECTING` for 15
seconds, refreshes host liveness and enters the normal reconnect path. This
transport timeout does not imply that the independent ptyhost process exited.

`/api/term/list` publishes fresh observed sessions plus Python-compatible pending
and source information. Explicit stop and force takeover cover managed and
external sessions through lifecycle discovery; resume creates a newly observed
instance rather than reclaiming an exited one.
