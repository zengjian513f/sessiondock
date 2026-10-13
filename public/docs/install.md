# Installation and local setup

Run these commands from the repository root with a Rust 2024 toolchain:

```sh
cargo build --release --locked -p sessiondock -p ptyhost
SESSIONDOCK_WEB_DIR=web ./target/release/sessiondock
```

The service listens on `127.0.0.1:8741`. The frontend is served directly from
`web/`, relative to the working directory. It is snapshotted at startup, so
restart the service after changing its assets. Node.js is not required.

## Try synthetic sessions

The checked-in fixtures are synthetic. They do not start a CLI:

```sh
SESSIONDOCK_CLAUDE_ROOT=crates/sessiondock/tests/fixtures/claude \
SESSIONDOCK_CODEX_ROOT=crates/sessiondock/tests/fixtures/codex \
SESSIONDOCK_GROK_ROOT=crates/sessiondock/tests/fixtures/grok \
./target/release/sessiondock
```

Copy fixtures to a private temporary directory before exercising writes.
For your own sessions, explicitly set the corresponding native root. Do not
commit that data or local environment values. SessionDock does not load `.env`
automatically. `sessiondock --check-config` validates configuration without
starting a listener or CLI.

## State and terminals

`SESSIONDOCK_STATE_DIR` enables persistent SessionDock metadata. Keep it private,
for example under an ignored `.runtime/` directory. Terminal hosting is optional:
configure `SESSIONDOCK_PTYHOST_DIR` explicitly and provide lifecycle storage and
a launcher configuration when enabling create/resume/stop operations. The source
configuration contract is `crates/sessiondock/src/config.rs`; example launcher
fixtures live in `tests/lifecycle_fixtures.py`.

Stopping SessionDock does not kill its independent terminal hosts. Inspect or
manage a host only with `ptyhost --dir <explicit-private-directory> ...`.

## Serve other devices

Keep the ordinary listener on loopback and place it behind an authenticated
reverse proxy. Configure `SESSIONDOCK_PUBLIC_HOSTS` for the accepted browser
hosts. Proxy routing can use a prefix such as `/sessiondock/`. Host and origin
checks are not user authentication; the proxy must enforce access control and
support WebSocket upgrades and SSE streaming.

The optional Hub is the `sessiondock-hub` binary in the same package. Use its
`--help` for registration and startup commands. Nodes use a dedicated authenticated
listener: `SESSIONDOCK_NODE_BIND`, `SESSIONDOCK_NODE_TOKEN_FILE`,
`SESSIONDOCK_NODE_ID_FILE` and `SESSIONDOCK_NODE_PEERS` form the node network
configuration. Keep tokens outside Git, and only permit trusted peers. An ID file
alone can provide stable local identity without enabling a Hub listener.

External services and resource collection remain separate configuration choices;
see [integrations](integrations.md).
