# Onboarding: reading order

Join the isolated Python → Rust AgentHub migration by reading in this order.
This file is a map, not a product spec.

## 1. What this repository is and is not

- [README.md](../README.md) — Independent Rust backend serving first-stage `legacy-web/`. Restricted native-history reads; **not** a full replacement and **not** Hub-node compatible. Vue `web/` stays frozen until stage two.
- [AGENTS.md](../AGENTS.md#scope-and-boundaries) — Isolated migration build, not production. Do not edit the sibling Python project. Native histories stay read-only. Loopback only; missing configuration fails closed. No production hosts, paid CLIs, or commits of credentials, paths, or runtime data.

## 2. Vocabulary

Read [glossary.md](glossary.md) before any contract. It defines UID/SID, event versus message, byte cursor versus semantic anchor, grants, native spans versus file references, and fork versus subagent.

## 3. Architecture and module map

- [architecture.md](architecture.md) — Default path: `legacy-web/` → Axum → bounded blocking pool → `sessions` parse/cache with SSE. Optional ptyhost, delivery, lifecycle, and files attach only after explicit directories.
- [module-map.md](module-map.md) — Generated map of every crate `src` `.rs` file.

## 4. Contracts by area

Read the first file in each group before its siblings. Full index: [docs/README.md](README.md).

### Sessions / read model

- [native-input.md](native-input.md) — Checked JSONL input, physical LF checkpoints, resident records; a private text span is not media or file authority.
- [history-pages.md](history-pages.md) — Finite gap pages versus the live checkpoint; grants bind canonical owner UID, agent, and producing view.
- [media.md](media.md) — Ephemeral image cache (lazy descriptors/blobs); no unconfigured disk or automatic remote fetch.

### Terminal / lifecycle

- [session-host.md](session-host.md) — Imported ptyhost; always pass an isolated development `--dir`.
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

- [delivery.md](delivery.md) — Codex/Claude receipt machines; **not** a working sender and does not complete M5.
- [delivery-store.md](delivery-store.md) — Isolated durable ledger; no sender or CLI.
- [delivery-engine.md](delivery-engine.md) — Exclusive owner of the store and both machines.
- [delivery-service.md](delivery-service.md) — Bounded async committed-outbox reads.
- [delivery-http.md](delivery-http.md) — Read-only HTTP after opening an existing ledger.
- [delivery-scope.md](delivery-scope.md) — `NativeScope` from the session store.
- [delivery-configuration.md](delivery-configuration.md) — Explicit directory; no implicit init.

### Files, diagnostics, capabilities, security

- [files.md](files.md) — Read-only M6 slice: explicit roots ∩ selected-view references.
- [diagnostics.md](diagnostics.md) — Bounded `POST /api/audit/browser`; unset keeps `audit:false` and 501.
- [capabilities.md](capabilities.md) — HTML / `/api/meta` flags; a missing key stays allowed.
- [security-model.md](security-model.md) — Trust boundaries: loopback, fail-closed, not a sandbox.

## 5. How work is organized

[BACKEND_MIRGRATION_PLAN.md](../BACKEND_MIRGRATION_PLAN.md) is the ledger.

- [§6 milestones](../BACKEND_MIRGRATION_PLAN.md#6-实施步骤与完成标准) — M0–M8 plus stage-two Vue; checkboxes are the completion standard.
- [§7 batch ledger](../BACKEND_MIRGRATION_PLAN.md#7-执行与验收记录) — Numbered batches: what shipped and how it was validated.
- [tests/plan_status.py](../tests/plan_status.py) — Summarizes §6 checkboxes and the latest §7 batch: `python3 tests/plan_status.py`.

## 6. Validate and run locally

- [validation.md](validation.md) — Table of every default suite in [tests/run_validation.py](../tests/run_validation.py). `--list` prints the plan; `--dry-run` does not execute. A full run takes the `target/` lock — skip it while other agents are building.
- [runbook-dev.md](runbook-dev.md) — Synthetic corpus, loopback bind, disjoint `SESSIONDOCK_*` directories. Never point roots at production CLI homes.

## 7. Delegating tooling tasks

[delegation.md](delegation.md) — Local `grok-4.6` may draft one self-contained, mechanically verifiable file. Human review is mandatory. Never delegate delivery, authorization, native semantics, or the ptyhost protocol.

## 8. First-day checklist (safe commands)

From the repo root. These are list/read-only: no build, no listener, no production data. Do not run `cargo` or a full `run_validation.py` until you own the `target/` lock.

```sh
python3 tests/plan_status.py
python3 tests/run_validation.py --list
python3 tests/run_validation.py --dry-run
python3 tests/check_docs_links.py
node --test tests/legacy_pure_contract.mjs
```
