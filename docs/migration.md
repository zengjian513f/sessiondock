# Python backend replacement

The first stage serves `legacy-web/` from the Rust service. `web/` remains the
Vue scaffold for a later frontend migration. `reference/legacy-web/` is a frozen
migration reference, preserved with its third-party licenses and never served.
The imported session host lives in `crates/ptyhost`; its local protocol remains
shared with the Python host client.

## Behavioral contract

Python is the behavioral reference. An additional Rust refusal must not become
product policy. Directory authorization, executable aliases, input sizes,
request fields, queue behavior, history parsing, file access and media follow
the corresponding Python operation. Actual Python request/protocol limits remain
in force; cache eviction and pagination may organize work without rejecting an
otherwise valid session.

The session inventory is lazy; opening one history does not make the whole list
parse every transcript. Views retain immutable snapshots and extend on append.
History-page cursors are separate from the live-message checkpoint. Unknown
native record kinds and malformed lines use the Python adapter's handling.

Search operates on the same semantic text shown in the session view and supports
lookaround and backreferences. File references and browser grants follow
`files.py` and `file_manager.py`; cwd itself is not a browser grant. Launcher cwd
is resolved using ordinary filesystem semantics without a separate directory
allowlist. Symlinked CLI installations remain usable.

Inactive sessions can be resumed through the selected node's advertised CLI
capabilities. Existing active consoles remain attached. The Hub validates its
own served asset version; an authenticated forwarded request does not compare
that version to a different node's local asset build.

## Identity and persistence

The product name and newly written preference namespace are SessionDock.
Compatibility wire names, old preference fallback reads and frozen references
retain the historical identifiers required by existing clients. Local metadata,
delivery and lifecycle state belong to the configured SessionDock deployment.
Credentials and production runtime data are never imported into this repository.

## Validation and deployment

Complete an implementation batch before running its consolidated tests. Validate
Rust, HTTP contracts, legacy browser flows and affected platforms with isolated
fixtures. Native-history comparisons are read-only. Model CLI tests use the
isolated configuration and inexpensive models specified in `AGENTS.md`.

A production bug fix includes deploying the validated batch to affected services,
preserving managed CLI processes and a rollback copy, then checking service
health and the reported user flow. Historical measurements and implementation
milestones live in [the migration ledger](../BACKEND_MIRGRATION_PLAN.md).

### Conversation attachment compatibility fix

The served composer chooses attachment reference separators from the destination
node (`path_style`, or drive/UNC detection for older nodes), so Windows uploads
insert `.\agenthub_attachments\…` even from a Linux/mobile browser. POSIX
references retain `./agenthub_attachments/…`. This intentional change from the
frozen frontend accompanies support for the composer's raw attachment endpoint;
the JSON file-manager upload completion endpoint remains supported.
