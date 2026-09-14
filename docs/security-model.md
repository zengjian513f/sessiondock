# SessionDock trust boundaries

SessionDock serves the existing browser application and follows the
backend's accepted inputs. Native histories remain read-only. Optional services
are enabled by their concrete runtime configuration; configuration is not used
to invent narrower product inputs.

## Authentication belongs to the reverse proxy

The ordinary server listens on loopback and has no user authentication or TLS
listener. Public deployments authenticate and terminate TLS at the reverse
proxy. `Host`, `Origin` and Fetch Metadata checks protect the browser boundary;
they do not create a user identity. Request parsing retains the HTTP protocol
boundaries, including per-line request/header handling.

## Loopback listener and Host / Origin

`security.rs::local_only` permits loopback authorities and configured public
authorities. Cross-site API requests and a mismatching `Origin` are rejected.
Hub protocol/token headers are rejected on the ordinary listener because Hub
traffic has its own authenticated node listener. API responses use `no-store`
and `nosniff` defaults.

## Node listener (hub traffic)

The node listener requires `SESSIONDOCK_NODE_BIND`, token file, node-id file and
peer networks together. It validates the TCP peer against the configured CIDRs,
then checks protocol `1` and the node token. It serves API routes without the
browser's loopback Host gate and never serves the static page.

The Hub registers nodes by literal IP inside its allowed networks. Both HTTP and
HTTPS are accepted; HTTPS uses the system trust store and validates the literal
IP certificate identity. The Hub never follows redirects and keeps credentials
out of public errors. See [hub.md](hub.md).

## Explicit configuration

Native Claude, Codex and Grok roots and the ptyhost directory remain explicit;
there is no CLI-home discovery. State, delivery, lifecycle, launcher, audit,
trash, node and Hub variables select their corresponding stores or services.
Use `sessiondock --check-config` to inspect the effective configuration.

The file read API follows the operator file-manager behavior and is always
available. File writes are available when terminal operation is enabled. A
launcher cwd is an execution choice, not file authorization. Configuration does
not require unrelated paths to be disjoint or require state/metadata directories
to have a Rust-only mode.

Metadata is reloaded for each operation. Missing, damaged or unsupported metadata
is treated as empty, and external edits may be observed and overwritten by a
later update. The metadata store has no exclusive lifetime
lock or permanent uncertain/frozen state. See [metadata.md](metadata.md).

## File access

File reads, navigation, attachments, uploads and enabled mutations follow the
file-manager routes. Paths are resolved with normal OS permissions and
the existing selected-session context used for presentation; cwd itself grants
no special authority. Uploads are streamed rather than encoded into JSON.
Symlink, rename, delete, trash and replacement behavior is documented in
[files.md](files.md) and [trash.md](trash.md).

## Media tokens

Native embedded images and file previews use process-local opaque tokens. Tokens
do not authorize arbitrary filesystem access and remote URLs are not fetched by
the server. Composer attachments use the normal file upload path; the browser
embeds uploaded paths in text and keeps media as preview metadata.

## Terminal authority

Browser ownership is an in-memory page lease over a verified ptyhost instance.
Claims, replacement, input and resize are serialized per host, and replacement
prevents a stale lease from beginning another write. The independent host token
and control framing are never exposed to the browser. Restarting the Web service
invalidates browser leases without stopping ptyhost children. The owner address
and device label shown in takeover prompts are display only: the node listener
takes the address from the hub's `X-Real-IP`, the browser listener only from
the TCP peer, the label comes from the claim's own `User-Agent`, and none of
them enters identity or authorization. See
[terminal-ownership.md](terminal-ownership.md).

## Launch allowlists

Launcher profiles choose executable, fixed arguments and environment. HTTP
chooses the profile/source and supplies the cwd or native session identity; it
does not supply an executable or arbitrary argv. New, resume, external-session
takeover and stop paths use the same observed instance identity and confirmation
semantics. See [lifecycle-launcher.md](lifecycle-launcher.md) and
[lifecycle-http.md](lifecycle-http.md).

## Diagnostics redaction

Audit and bug-report stores redact credentials and private path-like values as
documented in [diagnostics.md](diagnostics.md) and [bug-report.md](bug-report.md).
Their configuration controls persistence, without changing the accepted shapes
of unrelated APIs.

## debug_run views

`?debug_run=<id>` selects the test-session view. Missing or damaged
registry data behaves as an empty registry. An unknown run produces an empty view
rather than an authorization error. See
[read-model.md](read-model.md#debug_run-视图).

## 501 ledger (unimplemented writes)

Predecessor-only routes are not kept as migration stubs. Reliable send,
retry/discard, terminal input, attachments and file upload, trash operations,
external and managed stop, and forced takeover have concrete handlers. A 501 is
reserved for a capability that truly needs an unconfigured service, such as an
audit or bug-report backend that has not been enabled. Unknown routes remain 404
and malformed input remains 400.

## Assumption → enforcement → test

| Boundary | Enforcement | Coverage |
| --- | --- | --- |
| Ordinary listener stays local and browser-origin checked | `config.rs`, `security.rs` | HTTP/security suites |
| Node traffic requires peer, protocol and token | `api/node_auth.rs` | node-auth suites |
| Native history is read-only | sessions readers | history parity suites |
| Browser terminal writes require the current lease and instance | terminal ownership/service | terminal suites |
| Hub URLs remain literal IPs inside configured networks | hub registry/client | Hub suites |
| Diagnostics redact private values | audit and bug-report services | diagnostics suites |

OS permissions, reverse-proxy authentication and the local user's filesystem
authority remain deployment responsibilities.
