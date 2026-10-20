# Module map

This file is produced by `tests/module_map.py`. It maps every `.rs` file under
the crate `src` trees below with line counts and `mod` declarations.
Behavioral contracts live in the topic documents linked from [the index](README.md).
Regenerate:

```sh
python3 tests/module_map.py --write
```

## sessiondock

`crates/sessiondock/src`: 250 files, 104424 lines.

- `agy_sdk/`
  - `mod.rs` (1137 lines)
    - mods: `transcript`, `transport`
  - `transcript.rs` (178 lines)
  - `transport.rs` (719 lines)
- `api/`
  - `audit.rs` (79 lines)
  - `bug_report.rs` (825 lines)
  - `bug_report_sdk.rs` (176 lines)
  - `cleanup.rs` (30 lines)
  - `codex_sdk.rs` (424 lines)
  - `codex_sdk_actions.rs` (170 lines)
  - `codex_sdk_controls.rs` (61 lines)
  - `conversation.rs` (738 lines)
  - `events.rs` (37 lines)
  - `files.rs` (522 lines)
  - `final_screen.rs` (54 lines)
  - `health.rs` (51 lines)
  - `hub.rs` (1139 lines)
  - `lifecycle.rs` (1366 lines)
  - `media.rs` (150 lines)
  - `metadata.rs` (542 lines)
  - `mod.rs` (463 lines)
    - mods: `audit`, `bug_report`, `bug_report_sdk`, `cleanup`, `codex_sdk`, `codex_sdk_actions`, `codex_sdk_controls`, `conversation`, `events`, `files`, `final_screen`, `health`, `hub`, `lifecycle`, `media`, `metadata`, `native_sdk`, `node_auth`, `process_links`, `quotas`, `read`, `relations`, `runtime`, `search`, `shell_env`, `terminal`, `transfer`, `trash`, `trash_tree`
  - `native_sdk.rs` (417 lines)
  - `node_auth.rs` (103 lines)
  - `process_links.rs` (161 lines)
  - `quotas.rs` (22 lines)
  - `read.rs` (787 lines)
  - `relations.rs` (264 lines)
  - `runtime.rs` (562 lines)
  - `search.rs` (183 lines)
  - `shell_env.rs` (39 lines)
  - `terminal.rs` (1214 lines)
  - `transfer.rs` (975 lines)
    - mods: `scp`
  - `transfer/`
    - `scp.rs` (152 lines)
  - `trash.rs` (737 lines)
  - `trash_tree.rs` (163 lines)
- `assets.rs` (297 lines)
- `audit.rs` (370 lines)
  - mods: `intake`, `query`, `writer`
- `audit/`
  - `intake.rs` (265 lines)
  - `query.rs` (204 lines)
  - `writer.rs` (465 lines)
- `bin/`
  - `sessiondock-hub.rs` (204 lines)
  - `sessiondock-transfer.rs` (146 lines)
- `bridge/`
  - `claude.rs` (430 lines)
  - `codex.rs` (217 lines)
  - `live.rs` (235 lines)
  - `menus/`
    - `agy.rs` (314 lines)
    - `claude.rs` (712 lines)
    - `codex.rs` (637 lines)
    - `grok.rs` (566 lines)
    - `mod.rs` (18 lines)
      - mods: `agy`, `claude`, `codex`, `grok`, `opencode`
    - `opencode.rs` (663 lines)
  - `mod.rs` (14 lines)
    - mods: `claude`, `codex`, `live`, `menus`
- `bug_report/`
  - `mod.rs` (1053 lines)
    - mods: `worker`
  - `worker.rs` (466 lines)
- `claude_sdk/`
  - `actions.rs` (166 lines)
  - `hosted.rs` (563 lines)
  - `idle.rs` (60 lines)
  - `mod.rs` (882 lines)
    - mods: `actions`, `hosted`, `idle`, `protocol`, `transport`, `usage`
  - `protocol.rs` (623 lines)
  - `transport.rs` (464 lines)
  - `usage.rs` (201 lines)
- `cleanup.rs` (133 lines)
- `codex_sdk/`
  - `actions.rs` (87 lines)
  - `connection.rs` (319 lines)
  - `controls.rs` (64 lines)
  - `live.rs` (389 lines)
  - `mod.rs` (742 lines)
    - mods: `actions`, `connection`, `controls`, `live`, `observation`, `queue`, `receipts`, `usage`
  - `observation.rs` (71 lines)
  - `queue.rs` (401 lines)
  - `receipts.rs` (191 lines)
  - `usage.rs` (78 lines)
- `config.rs` (710 lines)
- `conversation/`
  - `cli_state.rs` (761 lines)
  - `input.rs` (495 lines)
  - `mod.rs` (1206 lines)
    - mods: `cli_state`, `input`, `paths`, `report_name`, `rewind`, `store`, `uploads`
  - `paths.rs` (73 lines)
  - `report_name.rs` (222 lines)
  - `rewind.rs` (134 lines)
  - `store.rs` (977 lines)
  - `uploads.rs` (248 lines)
- `delivery/`
  - `driver.rs` (1238 lines)
  - `mod.rs` (6 lines)
    - mods: `driver`, `target`
  - `target.rs` (204 lines)
- `error.rs` (68 lines)
- `files/`
  - `boundary.rs` (467 lines)
  - `grants.rs` (168 lines)
  - `media.rs` (113 lines)
  - `mod.rs` (317 lines)
    - mods: `boundary`, `grants`, `media`, `references`, `response`, `write`
  - `references.rs` (303 lines)
  - `response.rs` (519 lines)
  - `write.rs` (949 lines)
- `fingerprint.rs` (126 lines)
- `grok_sdk/`
  - `actions.rs` (366 lines)
  - `idle.rs` (67 lines)
  - `legacy_transport.rs` (242 lines)
  - `live.rs` (499 lines)
  - `mod.rs` (990 lines)
    - mods: `actions`, `idle`, `live`, `steering`, `transport`
  - `steering.rs` (100 lines)
  - `transport.rs` (486 lines)
    - mods: `legacy`
- `hub/`
  - `aggregate.rs` (948 lines)
  - `client.rs` (873 lines)
  - `groups.rs` (143 lines)
  - `identity.rs` (112 lines)
  - `mod.rs` (35 lines)
    - mods: `aggregate`, `client`, `groups`, `identity`, `namespace`, `nest`, `process_links`, `proxy`, `registry`, `resources`, `session_links`, `terminal_transport`, `transfer`, `relations`
  - `namespace.rs` (322 lines)
  - `nest.rs` (60 lines)
  - `process_links.rs` (202 lines)
  - `proxy.rs` (887 lines)
  - `registry.rs` (1439 lines)
  - `relations.rs` (86 lines)
  - `resources.rs` (340 lines)
  - `session_links.rs` (252 lines)
  - `terminal_transport.rs` (193 lines)
  - `transfer.rs` (1318 lines)
    - mods: `transport`
  - `transfer/`
    - `transport.rs` (158 lines)
- `hub_config.rs` (131 lines)
- `integrations.rs` (37 lines)
- `json_compat.rs` (69 lines)
- `lib.rs` (669 lines)
  - mods: `agy_sdk`, `api`, `assets`, `audit`, `bridge`, `bug_report`, `claude_sdk`, `cleanup`, `codex_sdk`, `config`, `conversation`, `delivery`, `error`, `files`, `fingerprint`, `grok_sdk`, `hub`, `hub_config`, `integrations`, `json_compat`, `lifecycle`, `list_sync`, `log`, `media`, `metadata`, `native_replay`, `native_sdk`, `observe`, `opencode_sdk`, `polls`, `quota_statusline`, `quotas`, `runtime`, `search`, `security`, `sessions`, `shell_env`, `state`, `terminal`, `transfer`, `trash`, `ui_events`, `session_links`
- `lifecycle/`
  - `agy_delete.rs` (211 lines)
  - `autobind.rs` (424 lines)
  - `clients.rs` (597 lines)
  - `launcher.rs` (1284 lines)
  - `mod.rs` (10 lines)
    - mods: `autobind`, `clients`, `launcher`, `model`, `models`, `service`, `store`, `agy_delete`
  - `model.rs` (631 lines)
  - `models.rs` (673 lines)
  - `service.rs` (1829 lines)
  - `store/`
    - `disk.rs` (211 lines)
    - `json.rs` (99 lines)
    - `mod.rs` (782 lines)
      - mods: `disk`, `json`
- `list_sync.rs` (390 lines)
- `log.rs` (137 lines)
  - mods: `runtime`
- `main.rs` (348 lines)
- `media.rs` (616 lines)
  - mods: `descriptors`, `discovery`, `file_media`, `formats`, `native_media`
- `media/`
  - `descriptors.rs` (299 lines)
  - `discovery.rs` (248 lines)
  - `file_media.rs` (201 lines)
  - `formats.rs` (9 lines)
  - `native_media.rs` (370 lines)
- `metadata/`
  - `disk.rs` (137 lines)
  - `mod.rs` (246 lines)
    - mods: `disk`, `model`, `relations`
  - `model.rs` (824 lines)
    - mods: `transfer`
  - `relations.rs` (180 lines)
  - `transfer.rs` (63 lines)
- `native_replay.rs` (219 lines)
- `native_sdk.rs` (969 lines)
  - mods: `input_queue`
- `native_sdk_queue.rs` (332 lines)
- `observe.rs` (217 lines)
- `opencode_sdk/`
  - `actions.rs` (265 lines)
  - `mod.rs` (911 lines)
    - mods: `actions`, `project`, `transport`
  - `project.rs` (221 lines)
  - `transport.rs` (203 lines)
- `polls.rs` (202 lines)
- `quota_statusline.rs` (98 lines)
- `quotas.rs` (326 lines)
- `runtime/`
  - `freeze.rs` (146 lines)
  - `mod.rs` (1426 lines)
    - mods: `freeze`, `process`, `process_links`, `procscan`, `spawn`
  - `process.rs` (466 lines)
  - `process_links.rs` (777 lines)
    - mods: `sync`
  - `process_links/`
    - `sync.rs` (176 lines)
  - `procscan.rs` (1143 lines)
    - mods: `activity`
  - `procscan/`
    - `activity.rs` (168 lines)
  - `spawn.rs` (330 lines)
- `search.rs` (881 lines)
  - mods: `cache`, `fold`, `prefilter`, `service`
- `search/`
  - `cache.rs` (865 lines)
  - `fold.rs` (208 lines)
  - `prefilter.rs` (163 lines)
  - `service.rs` (377 lines)
- `security.rs` (130 lines)
- `session_links.rs` (146 lines)
- `sessions/`
  - `agy.rs` (498 lines)
  - `history.rs` (87 lines)
  - `index/`
    - `agent_stops.rs` (267 lines)
    - `graph.rs` (908 lines)
    - `mod.rs` (1592 lines)
      - mods: `agent_stops`, `graph`, `names`, `native_state`, `summary`, `titles`
    - `names.rs` (200 lines)
    - `native_state.rs` (229 lines)
    - `summary/`
      - `agy.rs` (123 lines)
      - `claude.rs` (472 lines)
      - `codex.rs` (309 lines)
      - `grok.rs` (164 lines)
      - `mod.rs` (852 lines)
        - mods: `agy`, `claude`, `codex`, `grok`, `opencode`
      - `opencode.rs` (104 lines)
    - `titles.rs` (19 lines)
  - `media_projection.rs` (216 lines)
  - `mod.rs` (1524 lines)
    - mods: `history`, `index`, `agy`, `native_input`, `native_media`, `opencode`, `pages`, `views`, `providers`, `records`, `scope`, `sidebar`, `media_projection`
  - `native_input.rs` (336 lines)
  - `native_media.rs` (89 lines)
  - `opencode.rs` (628 lines)
  - `pages.rs` (662 lines)
  - `providers.rs` (1347 lines)
    - mods: `agy`, `claude`, `envelopes`, `grok`, `image_content`, `opencode`, `tools`
  - `providers/`
    - `agy.rs` (394 lines)
    - `claude.rs` (1024 lines)
    - `envelopes.rs` (275 lines)
    - `grok.rs` (139 lines)
    - `image_content.rs` (195 lines)
    - `opencode.rs` (178 lines)
    - `tools.rs` (1053 lines)
  - `records.rs` (426 lines)
    - mods: `native_images`, `native_records`, `scanner`, `summary_projection`, `string_reader`, `tool_envelopes`
  - `records/`
    - `native_images.rs` (532 lines)
    - `native_records.rs` (194 lines)
      - mods: `replay_source`
    - `native_records/`
      - `replay_source.rs` (184 lines)
    - `scanner.rs` (1003 lines)
    - `string_reader.rs` (255 lines)
    - `summary_projection.rs` (91 lines)
    - `tool_envelopes.rs` (244 lines)
  - `scope.rs` (108 lines)
  - `sidebar.rs` (92 lines)
  - `views/`
    - `body.rs` (273 lines)
    - `encoded.rs` (257 lines)
    - `mod.rs` (2344 lines)
      - mods: `body`, `encoded`
- `shell_env.rs` (224 lines)
- `state.rs` (221 lines)
- `terminal/`
  - `device.rs` (44 lines)
  - `final_screen.rs` (165 lines)
  - `input.rs` (165 lines)
  - `mod.rs` (32 lines)
    - mods: `device`, `final_screen`, `input`, `ownership`, `receipts`, `service`
  - `ownership.rs` (820 lines)
  - `receipts.rs` (196 lines)
  - `service.rs` (1286 lines)
- `transfer/`
  - `bundle.rs` (752 lines)
    - mods: `dependencies`
  - `claude_tools.rs` (95 lines)
  - `cleanup.rs` (107 lines)
  - `code_mode.rs` (331 lines)
  - `codex.rs` (682 lines)
  - `codex_ids.rs` (149 lines)
  - `codex_tools.rs` (236 lines)
  - `coordination.rs` (102 lines)
  - `dependencies.rs` (204 lines)
  - `environment.rs` (308 lines)
  - `files.rs` (1022 lines)
  - `grok_tools.rs` (167 lines)
  - `group.rs` (516 lines)
    - mods: `claude_tools`, `grok_tools`
  - `journal.rs` (106 lines)
  - `json_bytes.rs` (269 lines)
  - `mod.rs` (58 lines)
    - mods: `bundle`, `cleanup`, `code_mode`, `codex`, `codex_ids`, `codex_tools`, `coordination`, `environment`, `files`, `group`, `json_bytes`, `moving`, `native`, `progress`, `service`, `transport`, `references`
  - `moving.rs` (589 lines)
  - `names.rs` (176 lines)
  - `native.rs` (1029 lines)
  - `prefix.rs` (238 lines)
  - `progress.rs` (126 lines)
  - `references.rs` (422 lines)
  - `service.rs` (1203 lines)
    - mods: `journal`, `names`, `prefix`, `tool_requirements`
  - `tool_requirements.rs` (64 lines)
  - `transport.rs` (291 lines)
- `trash.rs` (880 lines)
  - mods: `deferred`, `manifest`, `plan`, `tree`
- `trash/`
  - `deferred.rs` (390 lines)
  - `manifest.rs` (333 lines)
  - `plan.rs` (144 lines)
  - `tree.rs` (265 lines)
- `ui_events.rs` (268 lines)

## process-links

`crates/process-links/src`: 6 files, 1754 lines.

- `agent.rs` (196 lines)
- `connections.rs` (358 lines)
- `engine.rs` (410 lines)
- `lib.rs` (270 lines)
  - mods: `agent`, `connections`, `engine`, `linux`, `resource_summary`
- `linux.rs` (265 lines)
- `resource_summary.rs` (255 lines)

## resource-agent

`crates/resource-agent/src`: 9 files, 3078 lines.

- `bandwidth.rs` (251 lines)
- `events.rs` (100 lines)
- `gpu.rs` (181 lines)
- `io_bpf.rs` (334 lines)
- `io_events.rs` (206 lines)
  - mods: `bpf`
- `main.rs` (35 lines)
  - mods: `bandwidth`, `events`, `gpu`, `io_events`, `memory`, `runtime_log`, `server`, `usage`
- `memory.rs` (53 lines)
- `server.rs` (1602 lines)
- `usage.rs` (316 lines)

## ptyhost

`crates/ptyhost/src`: 10 files, 3327 lines.

- `client.rs` (312 lines)
- `dsr.rs` (146 lines)
- `final_screen.rs` (98 lines)
- `guard.rs` (285 lines)
  - mods: `binding`
- `main.rs` (451 lines)
  - mods: `client`, `dsr`, `final_screen`, `guard`, `output`, `protocol`, `runtime_log`, `session`, `transport`
- `native_binding.rs` (153 lines)
- `output.rs` (213 lines)
- `protocol.rs` (168 lines)
- `session.rs` (1379 lines)
- `transport.rs` (122 lines)

## ptyhost-client

`crates/ptyhost-client/src`: 8 files, 2167 lines.

- `association.rs` (216 lines)
- `bound.rs` (244 lines)
- `dto.rs` (272 lines)
- `launch.rs` (259 lines)
- `lib.rs` (694 lines)
  - mods: `association`, `bound`, `dto`, `launch`, `native_binding`, `transport`, `wire`
- `native_binding.rs` (148 lines)
- `transport.rs` (98 lines)
- `wire.rs` (236 lines)

## ptyhost-screen

`crates/ptyhost-screen/src`: 3 files, 1058 lines.

- `grid.rs` (396 lines)
- `lib.rs` (8 lines)
  - mods: `grid`, `screen`
- `screen.rs` (654 lines)

## sdkhost

`crates/sdkhost/src`: 1 files, 427 lines.

- `main.rs` (427 lines)

## sdkhost-client

`crates/sdkhost-client/src`: 3 files, 497 lines.

- `lib.rs` (9 lines)
  - mods: `unix`, `unsupported`
- `unix.rs` (398 lines)
- `unsupported.rs` (90 lines)
