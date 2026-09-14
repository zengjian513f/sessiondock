# Explicit Codex name metadata

`SESSIONDOCK_CODEX_INDEX` optionally selects a Codex `session_index.jsonl` file.
Relative paths resolve from the server's working directory. The reader follows
ordinary file and directory aliases and does not write the name index.

`SessionStore::with_metadata_and_names(roots, metadata, index_path)` provides
the same optional behavior to internal callers. Existing `new` and
`with_metadata` constructors do not read a name index. Names are independent
of SessionDock's optional writable preference store.

## Compatibility

- Each JSON object with a nonempty string `id` and truthy `thread_name` assigns a name.
  The last qualifying row in **file order** wins, regardless of `updated_at`.
  Non-string names use Python-compatible scalar text. Missing/empty fields,
  malformed lines and non-object rows do not clear an earlier name.
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

Malformed JSON lines and invalid UTF-8 are skipped using the same per-line
fallback as Python. A missing file is an empty name index, so native titles
remain available. A complete final JSON line does not require a newline.

## Read boundary and budgets

The configured path is opened as an ordinary file, matching Python. Symlinks
and hard links are followed. The opened file and its stamp are checked before
and after reading so a mixed-version snapshot is never published. No native
file is written.

The complete file is read and all lines, IDs and names are considered; this
module adds no byte, line-count, ID-count or string-length admission limit.
Missing and malformed content falls back to native titles. Other filesystem
errors use a sanitized explanation; paths and OS error internals are not echoed.
An already frozen finite search may finish with its original consistent snapshot.

Metadata checks stat the configured file on hot read calls. Only changed content
is read and parsed; native inventory enumeration still uses the existing shared
refresh policy.

## Validation

Run module regressions with
`cargo test -p sessiondock --lib sessions::names --locked`.
Configuration isolation regressions run with
`cargo test -p sessiondock --lib config::tests --locked`.
After building the server, run
`python3 tests/names_parity.py --python-source PATH --browser` for
synthetic adapter differential checks and real legacy Chromium behavior.
The Python checkout is imported only as adapter code, with every native root,
name-index path, and cross-session lookup explicitly restricted to this tool's
temporary corpus. No Python index/state store, paid CLI, or real native data
is read. Linux runtime checks and any Windows cross-compilation results are
reported separately; neither implies Windows/macOS runtime acceptance.
