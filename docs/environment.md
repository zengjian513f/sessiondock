# Environment variables

This file is produced by `tests/env_reference.py`. Regenerate:

```sh
python3 tests/env_reference.py --write
```

## Node service (`sessiondock`, `Config::from_env`)

| Variable | Field | Required | Validation | Set by tests | Docs |
| --- | --- | --- | --- | --- | --- |
| `SESSIONDOCK_BIND` | `bind` | no (default) | invalid SESSIONDOCK_BIND | isolated_server, delivery_init.rs | [hub.md](hub.md), [runbook-dev.md](runbook-dev.md) |
| `SESSIONDOCK_WEB_DIR` | `web_dir` | no (default) |  | isolated_server | [deploy-hub.md](deploy-hub.md), [hub.md](hub.md), [runbook-dev.md](runbook-dev.md) |
| `SESSIONDOCK_CLAUDE_ROOT` | `roots.claude` | no (Option) | must not be empty; must be a directory | isolated_server | [liveness.md](liveness.md) |
| `SESSIONDOCK_CODEX_ROOT` | `roots.codex` | no (Option) | must not be empty; must be a directory | isolated_server | — |
| `SESSIONDOCK_GROK_ROOT` | `roots.grok` | no (Option) | must not be empty; must be a directory | isolated_server | — |
| `SESSIONDOCK_PTYHOST_DIR` | `ptyhost_dir` | no (Option) | Opt-in isolated terminal transport; must not be empty; must be a directory | isolated_server | [capabilities.md](capabilities.md), [lifecycle-http.md](lifecycle-http.md), [processes.md](processes.md), [runbook-dev.md](runbook-dev.md), [terminal-ownership.md](terminal-ownership.md) |
| `SESSIONDOCK_STATE_DIR` | `state_dir` | no (Option) | SessionDock-owned preferences and grants; must not be empty; must be a directory | isolated_server | [capabilities.md](capabilities.md), [error-codes.md](error-codes.md), [liveness.md](liveness.md), [read-model.md](read-model.md), [runbook-dev.md](runbook-dev.md) |
| `SESSIONDOCK_DELIVERY_DIR` | `delivery_dir` | no (Option) | Delivery state directory | isolated_server | [README.md](README.md), [capabilities.md](capabilities.md), [delivery-configuration.md](delivery-configuration.md), [runbook-dev.md](runbook-dev.md) |
| `SESSIONDOCK_LIFECYCLE_DIR` | `lifecycle_dir` | no (Option) | Creation receipts directory | isolated_server | [capabilities.md](capabilities.md), [lifecycle-http.md](lifecycle-http.md) |
| `SESSIONDOCK_LAUNCHER_CONFIG` | `launcher_config` | no (Option) | Server-owned adapter/launcher JSON, not browser input | isolated_server | [lifecycle-http.md](lifecycle-http.md) |
| `SESSIONDOCK_AUDIT_DIR` | `audit_dir` | no (Option) | Directory for browser diagnostics JSONL | isolated_server | [README.md](README.md), [capabilities.md](capabilities.md), [deploy-hub.md](deploy-hub.md), [diagnostics.md](diagnostics.md), [hub.md](hub.md) |
| `SESSIONDOCK_TRASH_DIR` | `trash_dir` | no (Option) | Directory for the session recycle bin | isolated_server | [README.md](README.md), [trash.md](trash.md) |
| `SESSIONDOCK_CODEX_INDEX` | `codex_index` | no (Option) | Explicit names file, separate from the Codex sessions root | no | [README.md](README.md), [codex-names.md](codex-names.md) |
| `SESSIONDOCK_PROC_ROOT` | `proc_root` | no (default) | Process table to scan; `/proc` by default and a synthetic tree in tests | no | [liveness.md](liveness.md), [validation.md](validation.md) |
| `SESSIONDOCK_GROK_ACTIVE` | `grok_active` | no (Option) | Optional override for `~/.grok/active_sessions.json` | no | [liveness.md](liveness.md) |
| `SESSIONDOCK_NODE_BIND` | `node_bind` | no (Option) | Second listener for Hub traffic; invalid SESSIONDOCK_NODE_BIND | node_auth.rs | [deploy-hub.md](deploy-hub.md), [deploy-macos.md](deploy-macos.md), [hub.md](hub.md), [security-model.md](security-model.md) |
| `SESSIONDOCK_NODE_TOKEN_FILE` | `node_token_file` | no (Option) | Shared hub credential (`--node-token-file`): 32–256 chars of `[A-Za-z0-9._~+/=-]`, read once at startup, never printed | node_auth.rs | [hub.md](hub.md) |
| `SESSIONDOCK_NODE_ID_FILE` | `node_id_file` | no (Option) | Persistent node identity (`--node-id-file`): 32 hex, minted `O_EXCL` 0600 on first start; an existing file is only read | node_auth.rs | [hub.md](hub.md) |
| `SESSIONDOCK_BUG_REPORT_DIR` | `bug_report_dir` | no (Option) | Bug-report bundles: an configured bundle directory | no | [bug-report.md](bug-report.md), [capabilities.md](capabilities.md), [error-codes.md](error-codes.md) |
| `SESSIONDOCK_BUG_REPORT_REPO` | `bug_report_repo` | no (Option) | The repository a bug-report worker investigates: the worker's cwd and the parent of `sessiondock_attachments/` | no | [bug-report.md](bug-report.md) |
| `SESSIONDOCK_SEARCH_CACHE_DIR` | `search_cache_dir` | no (Option) | Persistent search-text cache (`docs/read-model.md` "搜索") | no | [architecture.md](architecture.md), [performance.md](performance.md), [read-model.md](read-model.md), [runbook-dev.md](runbook-dev.md) |
| `SESSIONDOCK_SEARCH_CACHE_BYTES` | `search_cache_bytes` | no (default) | Byte cap of the on-disk search-text cache, least recently used entries evicted first (`SESSIONDOCK_SEARCH_CACHE_BYTES`, default 1 GiB); SESSIONDOCK_SEARCH_CACHE_BYTES must be an integer | no | [read-model.md](read-model.md) |
| `SESSIONDOCK_SEARCH_WORKERS` | `search_workers` | no (default) | Parse slots the search-text producer may use at once, shared by all searches and the warm-up; independent of the read worker pool (`SESSIONDOCK_SEARCH_WORKERS`, default `clamp(cpus/2, 2, 8)`); SESSIONDOCK_SEARCH_WORKE... | no | [architecture.md](architecture.md), [performance.md](performance.md), [read-model.md](read-model.md) |
| `SESSIONDOCK_SEARCH_WARMUP` | `search_warmup_secs` | no (default) | Seconds between low-priority background passes that refresh the search-text cache; 0 disables warm-up (`SESSIONDOCK_SEARCH_WARMUP`, default 300); SESSIONDOCK_SEARCH_WARMUP must be an interval in seconds from 0 (off) | no | [read-model.md](read-model.md) |
| `SESSIONDOCK_PUBLIC_HOSTS` | `public_hosts` | no (default) | Exact public authorities an authenticating reverse proxy forwards (`Host $http_host`); the Host gate accepts them beside loopback and keys Origin on them | hub_http.rs | [deploy-hub.md](deploy-hub.md), [hub.md](hub.md) |
| `SESSIONDOCK_HOSTNAME` | `hostname` | no (default) | The name the page, `<title>`, `/api/meta` and `/api/nodes` show for this machine: `SESSIONDOCK_HOSTNAME` when set, else the system host name, else `SessionDock`; SESSIONDOCK_HOSTNAME must be a non-empty display name | no | [runbook-dev.md](runbook-dev.md) |
| `SESSIONDOCK_NODE_PEERS` | `node_peers` | no (default) | Source networks the node listener accepts (strict CIDR list); every other peer is 403 before the token is looked at; SESSIONDOCK_NODE_PEERS must be a comma-separated CIDR list; SESSIONDOCK_NODE_PEERS: {reason} | node_auth.rs | [hub.md](hub.md) |
| `SESSIONDOCK_READ_WORKERS` | `pools` | no (default) | Pool, page, runtime and cache budgets: `SESSIONDOCK_READ_WORKERS` blocking readers (default `clamp(cores/2, 8, 32)`; probes and response permits derive from it), `SESSIONDOCK_HISTORY_PAGE_EVENTS` events per history pa... | no | [architecture.md](architecture.md), [performance.md](performance.md) |
| `SESSIONDOCK_HISTORY_PAGE_EVENTS` | `pools` | no (default) | Pool, page, runtime and cache budgets: `SESSIONDOCK_READ_WORKERS` blocking readers (default `clamp(cores/2, 8, 32)`; probes and response permits derive from it), `SESSIONDOCK_HISTORY_PAGE_EVENTS` events per history pa... | no | [history-pages.md](history-pages.md) |
| `SESSIONDOCK_ASYNC_WORKERS` | `pools` | no (default) | Pool, page, runtime and cache budgets: `SESSIONDOCK_READ_WORKERS` blocking readers (default `clamp(cores/2, 8, 32)`; probes and response permits derive from it), `SESSIONDOCK_HISTORY_PAGE_EVENTS` events per history pa... | no | [performance.md](performance.md), [read-model.md](read-model.md) |
| `SESSIONDOCK_CACHE_ENTRIES` | `pools` | no (default) | Pool, page, runtime and cache budgets: `SESSIONDOCK_READ_WORKERS` blocking readers (default `clamp(cores/2, 8, 32)`; probes and response permits derive from it), `SESSIONDOCK_HISTORY_PAGE_EVENTS` events per history pa... | no | [performance.md](performance.md), [read-model.md](read-model.md) |
| `SESSIONDOCK_VIEW_CACHE_MB` | `pools` | no (default) | Pool, page, runtime and cache budgets: `SESSIONDOCK_READ_WORKERS` blocking readers (default `clamp(cores/2, 8, 32)`; probes and response permits derive from it), `SESSIONDOCK_HISTORY_PAGE_EVENTS` events per history pa... | no | [performance.md](performance.md), [read-model.md](read-model.md) |
| `SESSIONDOCK_AST_CACHE_MB` | `pools` | no (default) | Pool, page, runtime and cache budgets: `SESSIONDOCK_READ_WORKERS` blocking readers (default `clamp(cores/2, 8, 32)`; probes and response permits derive from it), `SESSIONDOCK_HISTORY_PAGE_EVENTS` events per history pa... | no | [performance.md](performance.md), [read-model.md](read-model.md) |
| `SESSIONDOCK_FILE_ROOTS` | `file_roots` | no (default) | Legacy file-root values retained for configuration compatibility; they do not authorize or confine authenticated file reads | isolated_server | — |
| `SESSIONDOCK_FILE_WRITE_ROOTS` | `file_write_roots` | no (default) | Legacy write-root values may enable the write service for configuration compatibility; they do not authorize or confine target paths | isolated_server | [error-codes.md](error-codes.md) |

## Hub (`sessiondock-hub`, `HubConfig::from_env`)

| Variable | Field | Required | Validation | Set by tests | Docs |
| --- | --- | --- | --- | --- | --- |
| `SESSIONDOCK_HUB_BIND` | `bind` | no (default) | `SESSIONDOCK_HUB_BIND`, default `127.0.0.1:8742`; loopback only; invalid SESSIONDOCK_HUB_BIND; the hub has no authentication of its own; SESSIONDOCK_HUB_BIND must be a loopback address | no | [deploy-hub.md](deploy-hub.md), [hub.md](hub.md) |
| `SESSIONDOCK_PUBLIC_HOSTS` | `public_hosts` | no (default) | `SESSIONDOCK_PUBLIC_HOSTS`: authorities accepted besides loopback when the hub page sits behind the authenticated reverse proxy (same rule and parser as the node, `config::parse_public_hosts`) | hub_http.rs | [deploy-hub.md](deploy-hub.md), [hub.md](hub.md) |
| `SESSIONDOCK_HUB_NODES` | `nodes_file` | no (default) | `SESSIONDOCK_HUB_NODES`: the registry file (`hub-nodes.json`) | no | [deploy-hub.md](deploy-hub.md), [hub.md](hub.md) |
| `SESSIONDOCK_HUB_CACHE_DIR` | `?` | no (default) |  | no | [deploy-hub.md](deploy-hub.md), [hub.md](hub.md) |
| `SESSIONDOCK_HUB_NETWORKS` | `networks` | no (default) | `SESSIONDOCK_HUB_NETWORKS`: CIDR list a node may be registered from; invalid SESSIONDOCK_HUB_NETWORKS; invalid SESSIONDOCK_HUB_NETWORKS: {error} | no | [deploy-hub.md](deploy-hub.md), [hub.md](hub.md) |
| `SESSIONDOCK_WEB_DIR` | `web_dir` | no (default) | `SESSIONDOCK_WEB_DIR`, default `legacy-web`; served in hub mode; SESSIONDOCK_WEB_DIR must be an existing directory | isolated_server | [deploy-hub.md](deploy-hub.md), [hub.md](hub.md), [runbook-dev.md](runbook-dev.md) |
| `SESSIONDOCK_AUDIT_DIR` | `audit_dir` | no (Option) | `SESSIONDOCK_AUDIT_DIR`: where `hub.node.*.changed` records go; unset keeps the hub silent about display/order changes | isolated_server | [README.md](README.md), [capabilities.md](capabilities.md), [deploy-hub.md](deploy-hub.md), [diagnostics.md](diagnostics.md), [hub.md](hub.md) |
