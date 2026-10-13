# SessionDock

SessionDock is a Rust service and browser interface for CLI sessions. It includes
session history and search, managed terminals, files and attachments, and an
optional Hub for multiple nodes. The production frontend is plain JavaScript in
`web/`; it needs no Node.js or frontend build.

## Start locally

Use a Rust toolchain supporting edition 2024. The current development toolchain
is Rust 1.98.1; a minimum supported version has not yet been established.

```sh
cargo build --locked -p sessiondock -p ptyhost
cargo run --locked -p sessiondock --bin sessiondock
```

Open <http://127.0.0.1:8741>. With no configured session roots, the list is empty.
SessionDock does not automatically import your CLI home. See the
[installation guide](docs/install.md) for synthetic sample data, native roots,
and authenticated deployment.

## Components

| Component | Purpose | Required for a local page? |
| --- | --- | --- |
| `crates/sessiondock` and `web/` | Node service and browser UI | Yes |
| `crates/process-links` | Shared process attribution and protocol | Built with the node |
| `crates/ptyhost`, `ptyhost-client`, `ptyhost-screen` | Independent terminal hosts, client and screen model | For managed terminals |
| `sessiondock-hub` binary | Multi-node browser entry and federation | No |
| `crates/resource-agent` | Linux resource collector with an embedded BPF program | No |
| Labdesk, FileDock | External research and directory-browser integrations | No; not included |

A normal core build does not compile the resource collector and requires no BPF
compiler, collector service, or root access. Missing integrations do not prevent
the session list and built-in file APIs from working. Read
[optional integrations](docs/integrations.md) and the
[collector guide](docs/resource-agent.md) before enabling them.

## Development

Use isolated synthetic data and headless Chromium to exercise changed behavior.
See [validation](docs/validation.md), [contributor instructions](AGENTS.md), and
the [documentation index](docs/README.md). The public repository starts with a
reviewed source snapshot; it does not contain the private development history,
private fleet configuration, or production session data.

See the root license, when supplied, and [third-party notices](NOTICE.md).
A snapshot containing `PUBLICATION-DRAFT.txt` is a preparation artifact, not a
licensed public release.
