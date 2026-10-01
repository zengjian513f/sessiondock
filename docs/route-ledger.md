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
| Session resources | GET `/api/resources`, `/api/session/resources` (`uid`, `scope=direct` or `inclusive`); POST `/api/session/resources/probe`, `/api/resources/probe`; GET/POST `/api/process-links` | shared collector; session view resolves opaque UI UID to native identity, Hub aggregates execution nodes; link writes require node authentication |
| Runtime state | GET `/api/live`, `/api/term/list` | capability-gated |
| Preferences and timeline | GET/POST `/api/groups`, POST `/api/session/group`, `/api/session/star`, `/api/session/nest`, `/api/sessions/fork-visibility`, `/api/session/rewind` | capability-gated |
| Creation and control | POST `/api/term/create`, `/api/term/takeover`, `/api/term/kill`, `/api/term/bind`, `/api/term/discard`, `/api/term/backend`, `/api/session/stop`, `/api/session/freeze`; GET `/api/term/new-status`, `/api/term/complete-dir`, `/api/term/models` | capability-gated |
| Agent CLI versions | GET `/api/clients`; POST `/api/clients/update` (machine settings) | capability-gated |
| Terminal transport | POST `/api/term/claim`, `/api/term/send`, `/api/term/scroll`; WS `/api/term/attach` (`mode=grid` streams the server grid); GET `/api/term/grid/history` | capability-gated |
| Terminal recordings | GET `/api/term/records`; WS `/api/term/records/attach` (read-only replay, no lease) | capability-gated |
| Files and media | POST `/api/session/resolve-files`, `/api/session/attachment`, `/api/session/files/action`, `/api/session/files/upload`; GET `/api/session/file`, `/api/session/files`, `/api/media/{token}` | capability-gated |
| Conversation drafts | GET/POST `/api/session/conversation`; POST `/api/session/conversation/{send,check,restart,attachment,attachment/discard,queued/dismiss,import}`; GET `/api/session/conversation/drafts` | capability-gated |
| Trash | DELETE `/api/session/{uid}`; POST `/api/sessions/delete`, `/api/trash/restore`, `/api/trash/purge`; GET `/api/trash` | capability-gated |
| Diagnostics | POST `/api/audit/browser`, `/api/bug-report`; `debug_run` filtering | capability-gated |
| Hub | node registry and display settings; authenticated node proxy; HTTP/SSE/NDJSON/WS forwarding; UID/reference namespace conversion | hub mode |
