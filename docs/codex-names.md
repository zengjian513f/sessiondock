# Explicit Codex name metadata

`SESSIONDOCK_CODEX_INDEX` optionally selects an existing absolute
`session_index.jsonl` file. `SESSIONDOCK_CODEX_ROOT` must also be configured.
There is no default name-index path, parent-directory search, HOME discovery,
name-index write, or migration of an existing CLI profile. For example, point
both settings at separate synthetic files/directories under your development
fixture directory. The name index must be outside every configured static,
native-session, ptyhost, preference-state and file-access root. Configuration
validation resolves aliases only when checking overlap; it retains the exact
configured index path for the reader's no-follow checks. An alias to an index
inside a forbidden root cannot bypass isolation, and an alias to an otherwise
permitted external file is still rejected by the name reader.

`SessionStore::with_metadata_and_names(roots, metadata, index_path)` provides
the same optional behavior to internal callers. Existing `new` and
`with_metadata` constructors do not read a name index. Names are independent
of AgentHub's optional writable preference store.

## Compatibility

- Each JSON object with nonempty string `id` and `thread_name` assigns a name.
  The last qualifying row in **file order** wins, regardless of `updated_at`.
  Missing/empty fields do not clear an earlier name. Other fields are ignored.
- Titles collapse whitespace and are clipped at 110 Unicode characters, plus
  an ellipsis when truncated. `renamed_to` retains the complete original name.
- A valid `updated_at` supplies `renamed_at` and `renamed_to`; invalid or missing
  timestamps leave both null, but the name still overrides the title. Supported
  timestamps are RFC 3339, naive ISO date/time (assumed UTC), calendar dates,
  and Unix seconds/milliseconds. Responses normalize them to UTC milliseconds;
  Python emits equivalent instants in its local offset. Exotic Python
  `fromisoformat` forms (such as ISO week dates) are not emulated.
- A main session's own name wins. Otherwise, a validated fork inherits the
  nearest explicitly named main ancestor, or retains its native root title.
  Only a session's own entry supplies its `renamed_at`/`renamed_to` fields.
  Malformed or ambiguous topology retains the existing explicit warning.
- Attached agents retain their native labels and owner UID. Their
  `parent_title` follows the owner's renamed title; the main-thread index never
  overrides an agent label, even if an index entry matches that agent's ID.
- List, detail, frozen search, and SSE metadata use the same immutable name
  snapshot. A name update changes list signatures and view revisions but not
  native message content, byte offsets, semantic anchors, or cursor heads.
  It does not reparse unchanged rollout files. Search already in progress
  retains its captured names instead of mixing name revisions.

Deliberate differences from Python: corrupt rows/invalid UTF-8/incomplete tails
and nonempty non-string IDs/names are explicit errors, not silently skipped or
string-coerced. A complete final JSON line does not require a newline. Python
can manufacture a `/rename` message from the latest index timestamp; this
metadata-only implementation does not add such an event. Existing native
rename handling and native base-title clipping are not changed by this module.

## Read boundary and budgets

The explicitly configured path is walked with directory capabilities and
no-follow opens. Symlinks, Windows reparse points, hard-linked index files,
non-regular files, missing files, and inaccessible components fail closed.
The opened file and each parent edge are verified before and after bounded
reading; file replacement or identity changes during a read return 503. Cache
checks also verify the explicit path's component identities and permissions.
No path is reopened to produce response content and no native file is written.

Limits are 4 MiB per file, 50,000 lines (including blank lines), 10,000 distinct
nonempty IDs, 256 UTF-8 bytes per ID, and 16 KiB per full name. Exceeded budgets
return 413. Configuration-shape failures return 400. Missing, corrupt,
unreadable, unsupported-shape, or unstable configured files return 503 with a
sanitized explanation; filesystem paths and OS error internals are not echoed.
The last successfully published snapshot is retained internally but is **not**
served as a silent fallback after a detected name-index error. An already
frozen finite search may finish with its original consistent snapshot.

Metadata checks open/stat the single configured file and path components on
hot read calls. Only changed content is read and parsed; native inventory
enumeration still uses the existing shared refresh policy. Limits bound work,
not exact RSS or latency. Filesystem checks are not a cryptographic guarantee
against a privileged adversary able to falsify all identity/timestamp evidence.

## Validation

Run module regressions with
`cargo test -p sessiondock --lib sessions::names --locked`.
Configuration isolation regressions run with
`cargo test -p sessiondock --lib config::tests --locked`.
After building the server, run
`python3 tests/names_parity.py --python-source ../agenthub --browser` for
synthetic adapter differential checks and real legacy Chromium behavior.
The Python checkout is imported only as adapter code, with every native root,
name-index path, and cross-session lookup explicitly restricted to this tool's
temporary corpus. No Python index/state store, paid CLI, or real native data
is read. Linux runtime checks and any Windows cross-compilation results are
reported separately; neither implies Windows/macOS runtime acceptance.
