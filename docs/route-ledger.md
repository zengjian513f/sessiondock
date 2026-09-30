# HTTP route ledger

This is the compact route-family inventory used by `tests/route_ledger.py`.
It describes the standalone SessionDock product, not migration
progress. Concrete behavior and capability gates live in the module contracts
linked from [the documentation index](README.md).

| Area | Interfaces | Status |
| --- | --- | --- |
| Base | GET `/api/health`, `/api/meta`, `/api/nodes`; static pages | current |
| Sessions | GET `/api/sessions`, `/api/sessions/titles`, `/api/messages/{uid}`, `/api/messages/{uid}/page`, `/api/messages/{uid}/media-page`, `/api/session/input-history` | current |
| Background unread | POST `/api/sessions/unread` (read-only cursor summaries, node-scoped through Hub) | current |
| Sync and search | GET `/api/events` (lightweight UI changes), `/api/watch`, `/api/search` (including `progress=1` NDJSON) | current |
| Runtime state | GET `/api/live`, `/api/term/list` | capability-gated |
| Preferences and timeline | POST `/api/session/star`, `/api/session/nest`, `/api/sessions/fork-visibility`, `/api/session/rewind` | capability-gated |
| Group copies | POST `/api/session/clone/plan`, `/api/session/clone`; Hub POST `/api/session/transfer/clone` | capability-gated |
| Private transfer channel | POST `/api/session/transfer/manifest`, `/api/session/transfer/check`, `/api/session/transfer/reserve`, `/api/session/transfer/export`, `/api/session/transfer/receive`, `/api/session/transfer/status`, `/api/session/transfer/release` | authenticated node listener only |
| Private move handoff | POST `/api/session/transfer/switch`, `/api/session/transfer/activate`, `/api/session/transfer/retire` | authenticated node listener only |
| Creation and control | POST `/api/term/create`, `/api/term/takeover`, `/api/term/kill`, `/api/term/bind`, `/api/term/discard`, `/api/term/backend`, `/api/session/stop`, `/api/session/freeze`; GET `/api/term/new-status`, `/api/term/complete-dir`, `/api/term/models` | capability-gated |
| Agent CLI versions | GET `/api/clients`; POST `/api/clients/update` (machine settings) | capability-gated |
| Terminal transport | POST `/api/term/claim`, `/api/term/send`, `/api/term/scroll`; WS `/api/term/attach` (`mode=grid` streams the server grid); GET `/api/term/grid/history` | capability-gated |
| Terminal recordings | GET `/api/term/records`; WS `/api/term/records/attach` (read-only replay, no lease) | capability-gated |
| Files and media | POST `/api/session/resolve-files`, `/api/session/attachment`, `/api/session/files/action`, `/api/session/files/upload`; GET `/api/session/file`, `/api/session/files`, `/api/media/{token}` | capability-gated |
| Conversation drafts | GET/POST `/api/session/conversation`; POST `/api/session/conversation/{send,check,restart,attachment,attachment/discard,queued/dismiss,import}`; GET `/api/session/conversation/drafts` | capability-gated |
| Trash | DELETE `/api/session/{uid}`; POST `/api/sessions/delete`, `/api/trash/restore`, `/api/trash/purge`; GET `/api/trash` | capability-gated |
| Diagnostics | POST `/api/audit/browser`, `/api/bug-report`; `debug_run` filtering | capability-gated |
| Hub | node registry and display settings; authenticated node proxy; HTTP/SSE/NDJSON/WS forwarding; UID/reference namespace conversion | hub mode |
