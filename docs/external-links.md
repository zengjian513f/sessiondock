# External links

Conversation links accept `?sid=<source>:<native-id>` with optional `node=<node-id>`.
The selected node disambiguates copies across machines. Root sessions open their main
view. A native subagent ID resolves through `agent_items` and opens that subagent
under its owning root, including nested agents. The deep link takes precedence over
the last saved selection. `tests/session_deep_link_browser.py` covers root, child and
nested-child navigation on desktop/mobile with synthetic native histories, in both
sidebar modes.

When several rows match a native ID, they resolve only if all candidates converge
on one current row through explicit `continued_in` links within the same machine
and source. Independent copies, broken links or cycles that leave those candidates
ambiguous do not pick an arbitrary match. A sole matching row keeps its existing
behavior. Legacy UID links still resolve their row and the browser follows its
known continuation, as it does for other session navigation. See
[read model](read-model.md) for continuation evidence.

File links resolve to FileDock's machine-and-path entry; see [files](files.md).
The independent file service owns directory navigation and standalone previews.

## Sharing and old-link mapping

New links, templates and verification scripts use native session IDs, with `node`
when sharing a specific machine's copy. Build query values with URL encoding.
An internal SessionDock UID is a lookup key, not the native ID used in a new
share link. Resolve an old UID through that node's current session inventory;
do not derive the native ID from its path hash.

| Older form | Current form |
| --- | --- |
| `?sid=<internal-uid>` or a node-qualified UID | `?sid=<source>:<native-id>&node=<node-id>` from the resolved row |
| File service `file.html?node=ID&path=ABS` or `files.html?node=ID&path=ABS` | File service root `?node=ID&path=ABS`, normally `/files/?node=ID&path=ABS` |
| SessionDock file adapter with a session UID and relative reference | Resolve through the adapter, then share the resulting FileDock machine/absolute-path URL |

The session router retains UID fallback for old links; this is not the format
new scripts should generate. FileDock compatibility with an older entry is a
separate service concern. Historical receipts, verbatim conversations and
registration evidence keep their original links; use this mapping alongside them.

## Selection and navigation

Opening a session updates the address bar to this same shareable URL. A subagent
uses `?sid=<source>:<owner-native-id>/agent:<agent-id>`; standalone native subagent
IDs remain accepted. Refresh and browser Back/Forward restore the selected view.
Navigation reveals and selects its sidebar row, opening collapsed ancestors and
removing filters that would hide the target. Related-session links use this route
rather than a separate navigation scheme. Other query parameters and the proxy
base path are preserved.

Before a launch has native history, its URL uses `?sid=tmux:<host-name>`
with `node=<node-id>` on the Hub. Reload, sharing to a fresh browser and
Back/Forward wait for the terminal inventory and open that exact launch receipt,
without falling back to another saved session. Once its native binding appears,
the page replaces the URL with the native session link; the retained launch link
still resolves through its receipt. Node-qualified legacy launch UIDs remain
accepted. `tests/pending_session_link_browser.py` covers node/Hub navigation,
delayed inventory, missing receipts and desktop/mobile reloads with a fake CLI.

Selecting an already displayed session row preserves its folded children; only
the row's triangle expands or collapses that subtree. Restoring a selected parent
after reload also preserves its own fold. Revealing a hidden link target opens
only the ancestors needed to show the target row.

Subagent rows hang under their owning session in both flat and hierarchical
sidebar modes; the session row's triangle folds or expands them, and the
hierarchy toggle only controls whether spawned sessions indent. Opening an
agent deep link no longer switches the sidebar into hierarchical mode — it
reveals and selects the agent row in whichever mode is active.

## Retained move and copy lineage

The Hub exposes `POST /api/sessions/resolve` with `{"links":[{"sid":"source:native-id","node":"persistent-node-id"}]}`.
A link can instead supply a historical `host` name; prefer the persistent node from
its original URL. Results distinguish `found`, `missing`, `unavailable` and
`ambiguous`. Found sessions include their current title, canonical SID, scoped UID,
node, operation path (`via`) and whether that path contains a copy (`copied`).

All operation journals remain retained, including failures and cancellations;
only completed operations supply lineage edges. Each completed receipt records the
whole group's old native IDs and record UIDs mapped to target native IDs, including
subagents. Originals win while present. When absent, search completed descendants
breadth-first; prefer the earliest recorded operation at the same distance. Cycles
are visited once. Offline nodes or unresolved legacy mappings do not prove deletion:
return `unavailable` with accessible descendants offered for explicit selection.
Copies are labelled because their contents may have diverged.

The small graph is derived while loading existing operation journals and updated
when those journals are saved. There is no separate database, periodic scan or
continuous sync. Older receipts are backfilled on demand from the original node
operation, retaining the selected-session fallback when that is all the old receipt
proves. Node-local copies made without the Hub remain discoverable through that
node's retained operation journals when it is reachable. Transfers performed outside
SessionDock have no recorded edge and cannot be inferred.

Old Hub bookmarks use this resolver when their original row is absent or stale.
LabDesk and other callers keep their original links unchanged. Opening an old URL
runs the resolver inside SessionDock; callers need no migration lookup or extra
integration. Catalog identity, historical host and recorded URLs remain unchanged.
[The Chromium lineage suite](../tests/session_link_lineage_browser.py) covers copies,
deletions, subagents, restarts and offline origins; the bundle browser suite covers
links after real moves between private nodes.
