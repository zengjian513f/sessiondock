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

Selecting an already displayed session row preserves its folded children; only
the row's triangle expands or collapses that subtree. Restoring a selected parent
after reload also preserves its own fold. Revealing a hidden link target opens
only the ancestors needed to show the target row.

Subagent rows hang under their owning session in both flat and hierarchical
sidebar modes; the session row's triangle folds or expands them, and the
hierarchy toggle only controls whether spawned sessions indent. Opening an
agent deep link no longer switches the sidebar into hierarchical mode — it
reveals and selects the agent row in whichever mode is active.
