# Delivery directory configuration

`Config.delivery_dir` is optional and defaults to `None`.
`SESSIONDOCK_DELIVERY_DIR` supplies the same field through environment
configuration. There is no home-directory lookup, inferred default, directory
creation, ledger creation or automatic initialization. Both `Config::from_env`
and direct `Config::validate` use the same validation.

The Web binary uses the asynchronous `prepare_app` factory to open a configured
existing ledger through `DeliveryService`, including its durable two-provider
epoch recovery before serving requests. Startup failure never silently disables
the configured ledger. The synchronous `app`/`app_with_shutdown` factories reject
delivery configuration and direct library callers to `prepare_app` instead.
The resulting service advertises only `outbox_read`; it does not enable sending
or invoke an agent CLI. Main awaits the service's shutdown and writer-lock release.
Read [the HTTP contract](delivery-http.md) for exact native scope and response bounds.

There is a separate explicit initialization command:
`sessiondock --initialize-delivery ABSOLUTE_DIRECTORY`. It applies the same
configuration/isolation validation, requires a preexisting empty private
directory, initializes both providers' isolated delivery ledger through
`DeliveryEngine::initialize`, and exits without starting Web or any agent CLI.
It rejects preexisting initialized or partially initialized state; it is not a
repair, overwrite or automatic fallback path. Running the server without this
option never authorizes initialization merely because the environment variable
was supplied. `sessiondock --help` describes the command.

## Directory requirements

Supply an explicit absolute path to an existing independent directory. Empty,
relative, missing and non-directory paths are rejected, as are paths containing
parent-directory jumps. The directory and its existing ancestors must not be
symlinks or Windows reparse points. On Unix the selected directory must have
owner-only read/write/search permissions (`0700`); configuration does not
change its mode or contents.

The delivery directory cannot equal, contain or lie beneath any configured:

- frontend/static directory;
- Claude, Codex or Grok native-history root;
- ptyhost directory;
- AgentHub preference-state directory;
- Codex name-index file;
- file-access root.

Overlap checks compare both the configured spelling and resolved aliases.
An outside file-access/root alias pointing into delivery cannot bypass the
boundary. Lexical checks also reject a configured descendant that has not yet
been created. Resolving paths is only for comparison: the original
`delivery_dir` value remains unchanged for the store's own no-follow checks.

Configuration checks are advisory filesystem checks, not a replacement for the
delivery store's stronger open-time ownership, private-file, no-follow,
identity, schema and exclusive-writer guarantees. Filesystem state can change
after validation. An empty, correctly isolated directory can pass configuration
validation without being an initialized store; future runtime wiring must not
interpret a missing ledger as authorization to initialize one.

## Validation

Run `cargo test -p sessiondock --lib config::tests --locked` and
`cargo clippy -p sessiondock --all-targets --locked -- -D warnings`.
Tests use private temporary directories and child copies of the Rust test
executable to exercise environment parsing without changing the parallel test
runner's environment. They cover invalid shapes, every isolation boundary in
both directions, aliases, unchanged explicit paths, private directory modes
and the absence of any initialized ledger after configuration validation.
`cargo test -p sessiondock --test delivery_init --locked` separately covers
the explicit initialization command, invalid arguments and refusal to
overwrite existing state. These tests do not run real agent CLIs or
access production sessions. Unix permission/link runtime tests do not imply
Windows or macOS runtime validation.
