# Optional integrations

The default local session browser needs no Labdesk, FileDock, Hub, or privileged
collector. Each node and the Hub has its own explicit integration configuration.

| Setting | Default | Behavior |
| --- | --- | --- |
| `SESSIONDOCK_LABDESK_URL` | Unset | Research panels are disabled; no Labdesk requests |
| `SESSIONDOCK_FILEDOCK_URL` | Unset | External directory browsing is disabled; built-in single-file APIs remain available |
| `SESSIONDOCK_RESOURCE_AGENT_MODE` | `auto` | With a configured local node identity, use an available collector; retain process polling when unavailable |
| `SESSIONDOCK_RESOURCE_AGENT_MODE=off` | Explicit opt-out | Do not contact the collector |
| `SESSIONDOCK_NODE_ID_FILE` | Unset | Optional stable local node identity, also shared with the collector |

For an existing authenticated same-origin proxy, URL examples are `/labdesk/`
and `/files/`. Custom prefixes are supported. Configure integrations on the Hub
as well when users enter through it. An external service still owns its own
authentication, deployment and data. Configure matching node IDs across services;
a session's source and full session ID are meaningful only with its node identity.

Do not treat a failed request to a configured integration as proof that a session
is unregistered. The page retains a local unavailable/error state. Missing
collector samples are unavailable, not zero resource consumption. See the
[collector installation guide](resource-agent.md) for socket configuration,
Linux requirements and standalone builds.

Bug-report capture and automatic worker execution are separate optional
capabilities. Set `SESSIONDOCK_BUG_REPORT_DIR` and `SESSIONDOCK_BUG_REPORT_REPO`
together, plus private audit, metadata, attachment, terminal and lifecycle storage
for the current report composer. With `SESSIONDOCK_BUG_REPORT_WORKER` unset or
`false`, reports save diagnostics and do not launch a CLI. Set it to `true` to
also launch the selected worker; report controls remain hidden when the report
service prerequisites are absent. The source/model controls appear only for a
worker-enabled destination, including the Hub's per-node picker. A report
worker must follow the destination repository's instructions; enabling a report
button does not grant permission to publish or deploy changes.
