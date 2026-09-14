# SessionDock onboarding

Use this reading order when joining the standalone SessionDock project. This
file is a map, not a product spec; historical migration material is optional.

## 1. What this repository is and is not

- [README.md](../README.md) — Project scope, runtime and frontend direction.
- [AGENTS.md](../AGENTS.md) — Repository workflow and current development boundaries. Native histories stay read-only; do not commit credentials or runtime data.

## 2. Vocabulary

Read [glossary.md](glossary.md) first. It defines the shared terms.

## 3. Architecture and module map

- [architecture.md](architecture.md) — Default path: `legacy-web/` → Axum → session views/search/SSE, with ptyhost, delivery, lifecycle, files and Hub services.
- [module-map.md](module-map.md) — Generated map of every crate `src` `.rs` file.

## 4. Contracts by area

Read the first file in each group before its siblings. Full index: [docs/README.md](README.md).

### Sessions / read model

- [native-input.md](native-input.md) — Checked JSONL input, physical LF checkpoints, resident records; a private text span is not media or file authority.
- [history-pages.md](history-pages.md) — Finite gap pages versus the live checkpoint; grants bind canonical owner UID, agent, and producing view.
- [media.md](media.md) — Ephemeral image cache (lazy descriptors/blobs); no unconfigured disk or automatic remote fetch.

### Terminal / lifecycle

- [session-host.md](session-host.md) — Imported ptyhost; always pass a private development `--dir`.
- [terminal-ownership.md](terminal-ownership.md) — Browser lease state machine; existence does not enable endpoints.
- [terminal-identity.md](terminal-identity.md) — Lease versus instance guard; PID or SID alone is not incarnation proof.
- [lifecycle-store.md](lifecycle-store.md) — Private creation-intent receipts; no launch or native bind.
- [lifecycle-launcher.md](lifecycle-launcher.md) — Explicit adapter allowlist; no HTTP argv or home discovery.
- [lifecycle-service.md](lifecycle-service.md) — Bounded coordinator; does not infer SID/UID or enable reliable send.
- [lifecycle-http.md](lifecycle-http.md) — Create/status/cancel and pending consoles; default startup launches nothing.
- [lifecycle-integration.md](lifecycle-integration.md) — Linux integration contract; `terminal_create` stays false until configured.
- [lifecycle-binding.md](lifecycle-binding.md) — Operator assertion, not proof the CLI owns the native history.
- [processes.md](processes.md) — Read-only host observations; an empty list does not mean stopped.

### Delivery

- [delivery.md](delivery.md) — Codex/Claude receipt machines and Python-compatible delivery behavior.
- [delivery-store.md](delivery-store.md) — Durable delivery ledger and recovery semantics.
- [delivery-engine.md](delivery-engine.md) — Exclusive owner of the store and both machines.
- [delivery-service.md](delivery-service.md) — Serialized async committed-outbox reads and executor access.
- [delivery-http.md](delivery-http.md) — HTTP contracts backed by the configured delivery service.
- [delivery-executor.md](delivery-executor.md) and [delivery-codex-executor.md](delivery-codex-executor.md) — Claude/Codex send injection and native acknowledgment.
- [delivery-scope.md](delivery-scope.md) — `NativeScope` from the session store.
- [delivery-configuration.md](delivery-configuration.md) — Explicit directory; no implicit init.

### Files, diagnostics, capabilities, security

- [files.md](files.md) — Python-compatible file navigation, attachments, uploads and write operations.
- [diagnostics.md](diagnostics.md) — Bounded `POST /api/audit/browser`; unset keeps `audit:false` and 501.
- [capabilities.md](capabilities.md) — HTML / `/api/meta` flags; a missing key stays allowed.
- [security-model.md](security-model.md) — Trust boundaries: loopback, fail-closed, not a sandbox.

## 5. How work is organized

- [TODO.md](../TODO.md) — The only unfinished-work list.
- [route-ledger.md](route-ledger.md) — Compact route-family inventory checked against the router and legacy calls.
- Current behavior lives in the relevant `docs/` contract and its tests.

## 6. Validate and run locally

- [validation.md](validation.md) — Validation suites and runner options. A full
  run takes the `target/` lock; do not start one during another build.
- [runbook-dev.md](runbook-dev.md) — Synthetic corpus, loopback bind and optional service configuration.

## 7. Delegating tooling tasks

[delegation.md](delegation.md) — Rules for narrow Grok drafts. Review every
result; never delegate correctness-sensitive code.

## 8. First-day checklist (safe commands)

From the repo root. These are list/read-only: no build, no listener, no production data. Do not run `cargo` or a full `run_validation.py` until you own the `target/` lock.

```sh
python3 tests/run_validation.py --list
python3 tests/run_validation.py --dry-run
python3 tests/check_docs_links.py
node --test tests/legacy_pure_contract.mjs
```
