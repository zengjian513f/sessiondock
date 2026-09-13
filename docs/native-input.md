# Native input and structural scanning

SessionDock indexes native JSONL files without changing them. The index stores
line checkpoints and source stamps, and response reads verify that the selected
file still has the indexed stamp. Append and replacement invalidate cached
views so a response is never assembled from two file versions.

Native paths use ordinary Python-compatible file semantics. Symlinked parent or
leaf paths and hard links are readable. Lexical `.` and `..` components are not
accepted in an indexed absolute path, and an opened object must be a regular
file. These checks preserve the indexed path and version contract; they do not
add link-count, ancestry-depth or canonical-path admission rules.

Production indexing reads the complete stamped source and retains every line
checkpoint. Scanner allocation grows with the structure being parsed, while
large string bodies remain source spans.

## Structural JSON

The streaming scanner accepts the same JSON structure used by Python's
`json.loads`: nesting, node count, object key count, key length and number length
do not have separate SessionDock quotas. Duplicate object keys use the last
value. Syntax and UTF-8 errors remain errors.

Large strings become source spans so an embedded image does not have to remain
in the decoded AST. The span records source offsets, decoded length and digest;
materialization reopens the same stamped source and verifies the decoded value.
This mechanism is storage and version validation, not an input-size policy.

## Tool envelopes and native images

Stringified tool-result envelopes are recursively decoded for every candidate
that Python recognizes. There is no SessionDock-specific envelope-layer,
candidate-count or range-size quota, and the streaming parser continues through
all input blocks and lines.
Ordinary giant text is materialized and shown instead of turning an otherwise
valid record into an unsupported-history error.

Image authorization remains structural: only provider-recognized image fields
can produce native media. Text that merely resembles a data URL stays text.
Each decoded image retains Python's `media.MAX_ITEM` limit of 32 MiB. Invalid or
unsupported image candidates do not reject the surrounding session record.

Source offsets, hashes, MIME evidence and the current selected branch are
rechecked before a native span is served. A changed source returns a retryable
error rather than serving bytes from a different file version.

## Validation

Unit coverage lives under `sessions::native_input`, `sessions::records` and
`sessions::providers`. Synthetic HTTP coverage is in `tests/native_*.py`.
