# HTTP route ledger

This is the compact route-family inventory used by `tests/route_ledger.py`.
It describes the standalone SessionDock product, not Python-to-Rust migration
progress. Concrete behavior and capability gates live in the module contracts
linked from [the documentation index](README.md).

| Area | Interfaces | Status |
| --- | --- | --- |
| Base | GET `/api/health`, `/api/meta`, `/api/nodes`; static pages | current |
| Sessions | GET `/api/sessions`, `/api/messages/{uid}`, `/api/messages/{uid}/page`, `/api/messages/{uid}/media-page`, `/api/session/input-history` | current |
| Sync and search | GET `/api/watch`, `/api/search` (including `progress=1` NDJSON) | current |
| Runtime state | GET `/api/live`, `/api/term/list`, `/api/session/outbox` | capability-gated |
| Preferences and timeline | POST `/api/session/star`, `/api/sessions/fork-visibility`, `/api/session/rewind` | capability-gated |
| Creation and control | POST `/api/term/create`, `/api/term/takeover`, `/api/term/kill`, `/api/term/bind`, `/api/term/discard`, `/api/term/backend`, `/api/session/stop`; GET `/api/term/new-status`, `/api/term/complete-dir` | capability-gated |
| Terminal transport | POST `/api/term/claim`, `/api/term/send`, `/api/term/scroll`; WS `/api/term/attach` | capability-gated |
| Delivery | POST `/api/session/send`, `/api/session/draft-status`, `/api/session/outbox/retry`, `/api/session/outbox/discard` | capability-gated |
| Files and media | POST `/api/session/resolve-files`, `/api/session/attachment`, `/api/session/files/action`, `/api/session/files/upload`; GET `/api/session/file`, `/api/session/files`, `/api/media/{token}` | capability-gated |
| Trash | DELETE `/api/session/{uid}`; POST `/api/sessions/delete`, `/api/trash/restore`, `/api/trash/purge`; GET `/api/trash` | capability-gated |
| Diagnostics | POST `/api/audit/browser`, `/api/bug-report`; `debug_run` filtering | capability-gated |
| Hub | node registry and display settings; authenticated node proxy; HTTP/SSE/NDJSON/WS forwarding; UID/reference namespace conversion | hub mode |
