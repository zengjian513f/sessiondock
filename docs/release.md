# Release package and CI

## Linux package

```sh
python3 deploy/release.py [--ref REF] [--out DIR] [--check]
```

`deploy/release.py` builds one commit into
`target/release-packages/sessiondock-<version>-<short>-linux-<arch>.tar.gz`
(plus a `.sha256` file). It never builds the working tree: the commit is
extracted with `git archive` into a fresh temporary directory and compiled
there with `cargo build --release --locked`.

The archive holds one top-level directory with:

| Path | Content |
| --- | --- |
| `bin/` | `sessiondock`, `sessiondock-hub`, `sessiondock-transfer`, `ptyhost` |
| `web/` | `legacy-web/` as committed (served by `SESSIONDOCK_WEB_DIR`) |
| `README.md` | the repository README |
| `manifest.json` | commit, version, target triple, `rustc -V`, `source_date_epoch`, binaries |
| `SHA256SUMS` | SHA-256 of every other file |

`resource-agent` is not included: it is a separate Linux service with its own
deployment, and its build compiles an embedded BPF object with clang.

Reproducibility: build paths are removed with `--remap-path-prefix` (source,
Cargo home, work directory), `SOURCE_DATE_EPOCH` is the commit time,
incremental compilation is off, and the archive is written with sorted
entries, owner `0:0`, fixed modes (`0755` for binaries and directories,
`0644` otherwise), every mtime set to the commit time and a gzip header with
the same time and no file name. The same commit built with the same toolchain
(recorded in `manifest.json`) gives the same bytes. `--check` builds twice in
two different directories and exits 1 when the packages differ, keeping the
second as `<name>.second.tar.gz`. Verified on lyra (rustc 1.98.1) on
2026-10-06.

Fleet deployment does not use this package; it keeps using
`deploy/deploy.py` ([deployment](deployment.md)).

## Release acceptance

Before tagging `v*`, on a development node with Playwright and the CLIs logged
in (cygnus has Claude and Codex; lyra has Grok and Agy, with Playwright in its
`p311` Conda environment):

1. Commit everything and run the full sweep: `python3 tests/run_validation.py`
   (the Cargo lanes on lyra).
2. Run the paid real-CLI suites once:
   `python3 tests/run_validation.py --real-only --browser-jobs 1`. They use the
   cheap models from `AGENTS.md` at low effort with temporary homes, choose
   low effort in the page as a user would, and assert the model and effort the
   CLI actually recorded.
3. Record each suite as passed, failed, or not run with the reason (for
   example a provider usage limit) in the tag message. A suite that could not
   run is not a pass.
4. Push the tag; CI builds the package ([CI](#ci)).

The suites are kept out of the routine sweep and out of CI: they spend real
credits and need the operator's CLI logins.

| Suite | CLI | Covers |
| --- | --- | --- |
| `prompt_claude_real` | Claude (`claude-haiku-4-5-20251001`) | AskUserQuestion card pushed to the page, answered in the browser, answer in the native record, card cleared |
| `send_codex_startup_browser_real` | Codex (`gpt-5.6-luna`) | cold-start SEND from a new session: one exact native user message and a reply |
| `bug_report_codex_browser_real` | Codex (`gpt-5.6-luna`) | report dialog, native rename and the worker's first task |
| `send_grok_browser_real` | Grok (`grok-4.6`, low) | model and effort chosen in the picker, one composer SEND, one native user query and a reply |
| `agy_real_browser` | Agy, loopback synthetic gateway | models/argv, composer, native binding and history, menu guards, stopped-session continuation |
| `agy_interactions_real_browser` | Agy, loopback synthetic gateway | native approvals, questions, write-in answers and menu commands |
| `agy_tools_real_browser` | Agy, loopback synthetic gateway | native `view_file` text and PNG results opened in Chromium |

The Agy suites run the installed real CLI against a local synthetic model, so
they need no login and spend no credits; they still need the CLI installed.
A suite on a machine without its CLI fails rather than skipping.

Run on 2026-10-07 on lyra: `send_grok_browser_real` and the three Agy suites
passed (Agy 1.3.0, Grok 1.0.44).

Run on 2026-10-06: `prompt_claude_real` passed after the launcher began
clearing Claude Code's in-session markers (`CLAUDE_CODE_CHILD_SESSION` from a
service started inside a Claude Code session had turned transcript saving
off); both Codex suites reached the CLI with Luna at low effort, then failed
on the account's usage limit, so they were not run to completion.

## CI

`.github/workflows/ci.yml` runs on pushes to `main`, pull requests, `v*` tags
and manual dispatch:

| Job | Runner | Steps |
| --- | --- | --- |
| `rust` | Ubuntu 24.04, Windows, macOS | `cargo fmt --all --check` (Linux), `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo build --release --workspace --locked`; Linux installs `clang-18` for resource-agent |
| `checks` | Ubuntu 24.04 | `check_docs_links.py`, `check_agents_md.py`, `brand_names_check.py`, `env_reference.py --check` |
| `browser` | Ubuntu 24.04 | debug `sessiondock` and `ptyhost`, Playwright Chromium, then `legacy_browser`, `server_log_browser`, `terminal_final_screen_browser` and `history_browser`; validation logs are uploaded on failure |
| `release` | Ubuntu 24.04, `v*` tags only, after the others | `deploy/release.py --check`, package uploaded as the `linux-package` artifact |

CI compiles on the hosted Windows and macOS runners; it does not replace the
real-machine checks on vela and cetus, nor the full browser sweep
([validation](validation.md)), which stays on the development machines.
