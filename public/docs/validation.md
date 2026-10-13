# Browser validation

Build the core and install Python Playwright with Chromium:

```sh
cargo build --locked -p sessiondock -p ptyhost
cargo build --release --locked -p sessiondock -p ptyhost
python3 -m pip install playwright
python3 -m playwright install chromium
python3 tests/check_docs_links.py
python3 tests/check_agents_md.py
python3 tests/run_validation.py --list
python3 tests/run_validation.py --only legacy_browser,hub_browser
```

On Linux, Playwright may require system browser libraries (`playwright install
--with-deps chromium` installs them with the appropriate OS privileges). The
core application itself does not require privileged execution.

Use `--only` to select suites covering a change. Tests drive headless Chromium
against loopback servers and synthetic data in private temporary directories.
They do not require production session roots or a private fleet. Suites ending
in `_real` are opt-in, need the corresponding CLI installed and can spend credits;
the normal runner excludes them unless `--include-real` or `--real-only` is used.

`run_validation.py` also has Rust build/lint lanes. Its full cross-platform sweep
needs additional tools such as `cargo-xwin`; use the targeted browser commands
above for local feature validation. The source-tree brand checker requires a Git
checkout; in a plain exported directory, use browser and documentation checks
directly until you initialize the independent public repository.

## Included suites and utilities

The exporter appends the complete Python entry-point inventory below. This list
records available tools, not a claim that every suite was run for this release.
