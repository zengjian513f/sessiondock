# Native history tag envelopes

SessionDock interprets tags only in their native provider, record role and
structural context. This is display projection: native histories, CLI input,
media authority and physical byte checkpoints remain unchanged. The semantic
anchor detects changed projected history after a service restart.

The history audit found the following families. The projection intentionally
extends the frozen Python oracle (**DELTA**); `tests/native_tags_browser.py`
asserts each difference with synthetic histories through the real legacy UI,
API, title and search paths. Historical source-code examples and unknown or
incomplete structures remain text, not rejected input.

| Family | Interpretation |
| --- | --- |
| Codex `recommended_plugins` | A complete user envelope beginning with the native unavailable-plugin-list introduction is injected context. Exclude it from history, fallback titles and search. Classify each native text block before joining bundled plugin, AGENTS and environment injections; preserve adjacent user blocks in either order. Inline quotations and incomplete envelopes stay literal |
| Claude `fork-boilerplate` | A complete native worker-fork introduction in a subagent user text block is context. Drop only that block, preserving the actual directive in adjacent blocks. Main-session user text stays literal |
| Grok `user_query` + `skill_information/skills_referenced/skill` | The optional complete skill appendix is injected context outside the user's query. Extract the query; its protocol-looking body remains literal user text. Unknown trailing text prevents unwrapping |
| Codex `image` | Remove text delimiters only when they enclose contiguous native image blocks in a user content array. Keep typed images and surrounding text. A textual path does not authorize any read; text-only and incomplete examples stay literal |
| Claude `task-notification/event` | Keep both `event` and `result` in expandable notification details, including entity-decoded content. Preserve task ID, tool-use ID, output-file, note, usage and worktree fields. Unknown fields stay literal in details |
| Claude `tool_use_error` | Unwrap the complete tool-output envelope; native error metadata remains authoritative |
| Claude `persisted-output` | Unwrap the complete tool-output envelope, keeping the large-output notice, saved-file path and preview. Do not fetch that file automatically |
| Claude TaskOutput `retrieval_status/task_id/task_type/status/output/error/exit_code` | Label the complete known field sequence. A retrieval timeout can coexist with a running task and is not converted to a process failure. Unknown fields retain the original sequence |
| Claude tool `system-reminder` | Label complete leading/trailing reminder blocks, keeping their bodies and the intervening tool output. Handle combinations with output envelopes. Inline references and fenced examples stay literal |
| Grok `workspace_result` | Keep workspace provenance and the complete result body, removing the native outer wrapper only |
| Grok `task-id/task-type/output-file/status/summary/output/exit-code` | Label the complete background-task result sequence; retain state and output location. Unknown structures stay literal |

Notification usage fields include `subagent_tokens`, `tool_uses`, `duration_ms`,
`agent_count`, `agents_done`, `agents_error`, `agents_skipped` and
`agents_empty_result`. Worktree fields are `worktreePath` and `worktreeBranch`.
Their textual labels carry the recorded values without changing CLI state.

Already-covered context remains covered by its existing semantics:

- Codex developer messages, environment/rule injections and
  `content_item_kinds: ["goal.internal_context"]` (including
  `codex_internal_context/objective`) are not ordinary conversation.
- Claude `isMeta` records, including peer `agent-message`, skill-format and
  autonomous-loop notices, remain context. The `pasted_content` rule is
  documented in [history-pages.md](history-pages.md#fixture-checks).
- Grok system preambles, user-info/rule injections and `synthetic_reason`
  reminders (including `monitor-event`) retain their existing filtering.
- Native local commands, task notifications and Codex abort prefixes retain
  their existing handling. User-authored `<task>`, HTML, XML, generics and
  quoted protocol names are not globally stripped.

Search still uses the existing conversation roles: tool output and notification
metadata do not become conversation-search text. User-visible conversation,
its search body and fallback titles agree on the user-envelope rules. The
frontend's existing tool-output and event-detail disclosures display the
normalized text without introducing another interpretation of the tags.

## Validation

```sh
cargo build -p sessiondock --locked
python3 tests/native_tags_browser.py
python3 tests/history_parity.py
python3 tests/history_browser.py
```

The tag suite clicks every affected session, expands notification details and
tool groups, loads the typed image, switches to a subagent, types and submits
searches, and observes a live append. Positive cases are paired with literal,
malformed or unknown-field controls. Test histories remain byte-for-byte
unchanged except for the suite's explicit synthetic append. No production
histories or paid CLIs are used.
