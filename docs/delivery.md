# Delivery state and behavior

SEND is the conversation service's ([conversation.md](conversation.md)): it
writes to the CLI and waits for the native echo. The delivery ledger is a
read-only archive of receipts the retired reliable-send executor recorded
before that service existed; `GET /api/session/outbox` (see
[delivery-http.md](delivery-http.md)) and bug reports still read it, nothing
writes it. [delivery-engine.md](delivery-engine.md) describes the ledger's
domain and [delivery-store.md](delivery-store.md) its persistence.

## Baseline

Submission and confirmation matched the frozen queue, protocol, audit and
native bridge code.

## Interface and state transitions

The Codex and Claude machines retain request identity and state. The engine
persisted every transition before acting on the terminal. Native evidence, not
a transport return, established queueing, acceptance and completion.

## Persistence, ordering, privacy and bounded resources

The request ID covers the full payload. An identical retry reuses its receipt;
a different payload conflicts. Rust adds no ledger or JSON quota. Media remains
opaque metadata; the prompt already contains uploaded-file references.

## Validation

Run the affected workspace, HTTP, differential, and browser suites. Ordinary
tests use synthetic histories and fake CLIs. Real CLI tests use temporary
configuration and the models in `AGENTS.md`. Record results with the change;
put unfinished work in `TODO.md`.

## Claude: independent pure queue/turn domain

Claude shares persistence with Codex but keeps its own receipt, confirmation
and interruption rules.

### What the current Claude bridge actually proves

`AskUserQuestion` hooks describe a dialog. Only matching native queue or
transcript evidence proves prompt acceptance.

### Four different milestones must remain distinct

Receipts distinguish persistence, transport, native acceptance and completion.

### Drafts, interruption and parent/child boundaries

Operations use the selected native view and its terminal. That view supplies
parent and child identity. Interruption follows composer behavior.

### Recovery, cancellation, dismissal and remaining integration

Restart reconciles pending work with native state. Persistence retries never
repeat paste or Enter. Cancellation keeps receipt identity. Storage errors and
uncertain CLI outcomes remain distinct.

## Live questions and approvals

Claude question cards come from `sessiondock claude-hook`; Codex approvals come
from the managed TUI screen. Main-session message and watch responses expose
them as `prompt`. A native answer clears the card.

The page answers through `/api/term/send` under its console lease. The bridge
never writes to the CLI, takes a lease or acknowledges delivery. Hook files and
generated settings are private and atomic. Codex prompt recognition must match
the frozen bridge. See [terminal-input.md](terminal-input.md) and
[lifecycle-launcher.md](lifecycle-launcher.md).

Validate with `tests/claude_prompt_suite.py`. Run the
real Claude and Codex prompt suites only under the real-CLI policy in
`AGENTS.md`.
