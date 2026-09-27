# External links

Conversation links accept `?sid=<source>:<native-id>` with optional `node=<node-id>`.
The selected node disambiguates copies across machines. Root sessions open their main
view. A native subagent ID resolves through `agent_items` and opens that subagent
under its owning root, including nested agents. The deep link takes precedence over
the last saved selection. `tests/session_deep_link_browser.py` covers root, child and
nested-child navigation on desktop/mobile with synthetic native histories.

File links resolve to FileDock's machine-and-path entry; see [files](files.md).
The independent file service owns directory navigation and standalone previews.

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
