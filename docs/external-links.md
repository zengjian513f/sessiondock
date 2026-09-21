# External links

Conversation links accept `?sid=<source>:<native-id>` with optional `node=<node-id>`.
The selected node disambiguates copies across machines. Root sessions open their main
view. A native subagent ID resolves through `agent_items` and opens that subagent
under its owning root, including nested agents. The deep link takes precedence over
the last saved selection. `tests/session_deep_link_browser.py` covers root, child and
nested-child navigation on desktop/mobile with synthetic native histories.

File links resolve to FileDock's machine-and-path entry; see [files](files.md).
The independent file service owns directory navigation and standalone previews.
