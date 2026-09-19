# Delivery directory configuration

`SESSIONDOCK_DELIVERY_DIR` supplies optional `Config.delivery_dir`.
`prepare_app` opens the configured delivery service before serving requests.
A configured service's startup error is returned to the caller.

`sessiondock --initialize-delivery DIRECTORY` creates an initial delivery
ledger without starting HTTP or a CLI. Normal startup opens the ledger and
initializes it when it is missing.
The command preserves an already initialized ledger.

## Directory requirements

The selected directory is created as needed. Relative paths, parent components,
symlinks and ordinary permissions follow filesystem semantics. Other service
paths do not form a delivery-directory authorization whitelist. OS access errors
are reported directly by the operation.

## Validation

Default check is `python3 tests/check_config_suite.py` and the delivery HTTP
suites. Do not run crate unit tests unless the user asks. See
[delivery-store.md](delivery-store.md), [delivery-service.md](delivery-service.md).
