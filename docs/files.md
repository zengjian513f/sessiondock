# Session files and directory browsing

The file service follows the Python `files.py`, `file_manager.py` and server
`_file_access` contract. It supports referenced-file reads, directory browsing,
file previews/downloads, new files/directories, rename, move, upload and scoped
trash. Unimplemented copy/archive/restore/job-retry operations remain explicit
501 responses.

## References and browser grants

A direct file reference must occur in the complete selected session/agent
history. Paths in tool arguments, Markdown links, quoted paths and line-number
suffixes use the Python reference rules. A basename must itself be mentioned;
its existing candidates are canonicalized and must identify one distinct file.
Referenced directories supply direct-child candidates, without recursive search.
Unavailable candidates are skipped; cwd supplies relative-path context and does
not itself grant access.

Opening a directory browser first requires a valid session view and a directory
reference. The service persists that grant under the exact UID, agent and
reference in `file-browser-grants.json` in the private state directory. The
session is validated on every request. A granted browser can navigate to other
absolute paths, including parents up to the filesystem root, even after its
original entry directory is renamed or removed. A new UID/agent/reference
combination must establish its own grant.

Referenced-file reads are available for authenticated requests. Terminal mode
also enables file mutations. Browser grants permit navigation throughout the OS
filesystem; actual OS permissions apply. Listings report the volume root in
`root`, and `parent` becomes null there.

## Module interface

`FileScope { uid, agent, cwd, messages }` comes from a validated selected view.
`FileService::target` resolves direct references. `browser_target` and
`browser_anchor` establish or reuse the browser grant. `navigation` is the
lower-level path resolver and must only be called after authority is established.
`with_grants` opens the optional durable grant store; without a state directory,
grants last for the lifetime of the process.

## Filesystem checks and platform behavior

Reads resolve symbolic links like Python `Path.resolve()`, including symlink
ancestors followed by `..`. Every resulting component is then opened through
checked directory handles with no-follow operations. Reads retain the actual
file handle and recheck ancestor/leaf identity and metadata during transfer;
they do not reopen a display path to read bytes. This detects a replaced parent
or file without making symlinks themselves forbidden. File opens are nonblocking
so a swapped-in FIFO cannot turn an ordinary-file read into an indefinite wait.
Regular hard-link aliases are readable. Devices, sockets and pipes are rejected.

Directory-browser `mode=info` uses the same parent resolution while retaining
its named leaf: a valid browser grant can inspect a dangling symlink and its
literal `link_target`, even if the original entry directory was removed. The
link's metadata and suffix describe it; a regular target still supplies a checked
text/media preview. Direct file references and byte downloads continue requiring
a resolvable target.

Mutations normalize the input and resolve its parent directory, preserving the
leaf itself. Renaming or trashing a symlink moves that link, including a dangling
link, without modifying its target. A hard-link alias can be renamed, moved or
trashed while other aliases remain intact. Write operations retain and verify
the checked parent handles. An in-progress upload cannot silently switch to a
replacement destination directory, while a fresh request can resolve that new
directory normally.

Python's `Manager.guard` exclusions remain: modifying the filesystem root, the
user's home directory itself, or the private file-manager state (including its
ancestors or descendants) is refused. Navigation and ordinary reads are not
restricted by these mutation checks. Names follow Python's single-component
rules: nonempty, at most 255 bytes, not `.`/`..`, and no slash, backslash or NUL.
Input paths use the Python 4096-character/NUL checks; relative references use
only the session cwd. `~/` references use the process's configured home.

## Representation and resource policy

| Operation | Behavior |
| --- | --- |
| `POST /api/session/resolve-files` | Up to 256 requested references; resolved targets and explicit per-reference errors. |
| `GET /api/session/file` | Exact selected-history reference; raw response, info, preview or download. |
| `GET /api/session/files` | Directory browser grant plus optional absolute navigation; sorted pages of 1–500 entries. |
| `mode=info` | 1 MiB text preview, replacement characters for invalid UTF-8, explicit truncation; NUL-containing text is unsupported. |
| Raw file open | Python's 32 MiB limit; HTML/SVG/scripts are served as text rather than executable origin content. |
| Download/media preview | Streamed from checked handles. PDF preview verifies the PDF marker as Python does. |
| HEAD/Range | Representation headers without HEAD body; one byte range, explicit 416 for invalid/unsatisfiable ranges. |

Directory enumeration is paginated. Reference scanning traverses the complete
selected history, using an explicit stack for nested JSON. Basename and directory
indexes are built once per batch. Media references instead resolve directly
against the session cwd, with Python-compatible percent decoding and home
expansion. Name/type sorting currently uses Unicode lowercase, where Python uses
Unicode casefold.

Responses retain no-store, nosniff, safe content dispositions and the existing
content-security policy. Non-PDF content is sandboxed; PDF preview permits the
browser's isolated PDF viewer. Checked handles do not provide filesystem snapshot
semantics or protect against an administrator concealing concurrent inode edits.
Linux acceptance does not establish Windows/macOS runtime behavior.

## Write operations

`POST /api/session/files/action` accepts `mkdir`, `new-file`, `rename`, `move`,
`delete`, `upload` and `cancel`. Every action is tied to the validated browser
scope. `mode=jobs` lists that scope's jobs, up to Python's 100 displayed rows.
Jobs belonging to another scope remain inaccessible.

Conflict policies are `error`, `skip`, `keep` and `replace`. Replace first moves
the old destination into scoped private trash, preserving its content, then
publishes the new entry. Regular-file publication uses no-clobber hard links
or an exclusive copy fallback. Existing-file, directory and symlink moves use atomic
`renameat_with(NOREPLACE)` through the retained parent handles on Linux/macOS;
Windows uses `NtSetInformationFile(FileRenameInformation)` with the held source/destination
handles and `ReplaceIfExists=false`. A concurrent destination
creation is a conflict, not permission to overwrite it.

Cross-device moves copy into a temporary directory on the destination device,
verify the source, and atomically publish the completed entry. The original
then goes into scoped trash so it remains recoverable, matching Python.
Cross-device trash also copies before removing the original. Recursive copying
preserves link leaves (including dangling/absolute links), file permissions and
timestamps; checked source trees are revalidated before removal.

Trash lives under `<state>/file-trash/<id>/` with a manifest. An explicit delete
moves an entry there rather than unlinking it. Upload partials live in a private
`.sessiondock-upload/` directory under the actual destination directory so
publication can stay on that filesystem; they never use `/.sessiondock-upload`
merely because navigation now reaches the volume root.

Defaults match Python: up to 2000 selected items, 1 TiB per upload, 8 MiB per
chunk, 512 MiB per conversation or bug-report attachment and 100,000 keep-name attempts. Upload jobs remain available until explicitly handled. Upload offsets,
optional hashes and declared sizes are validated; duplicated acknowledged chunks
are idempotent, conflicting chunks fail without rewriting accepted bytes.

Conversation attachments use `POST /api/session/attachment?uid=...&name=...`
with the raw file body (including files whose MIME type is `application/json`).
The server resolves the selected native session's cwd and saves files under
`sessiondock_attachments/<id>/`; no client cwd or prior file-browser grant is needed.
Write service configuration is still required. An optional `id` groups a draft's
files, identical files are reused, and different content with the same name gets
a numbered suffix. Metadata records the resulting path when configured. The
existing JSON upload-job completion contract remains available without a query
uid, and `uid=bug-report` retains its dedicated configured repository.

Upload responses include the destination node's `path_style` (`windows` or
`posix`) and a native `relative_path`. The composer inserts Windows paths as
`.\sessiondock_attachments\<id>\<name>` and POSIX paths as
`./sessiondock_attachments/<id>/<name>`, regardless of the browser's OS. For older
nodes without `path_style`, Windows drive and UNC absolute paths identify the
convention. Windows canonical drive paths are returned with the ordinary drive
spelling, so `\\?\C:\…` does not defeat configured-root comparisons. Spaces and Unicode names stay intact; these are references in the
message, not shell commands.

## Validation

`cargo test -p sessiondock --lib files:: --locked` covers reference/agent
isolation, cross-directory navigation, grant persistence, symlink/`..` resolution,
deep paths/tool arguments, large directory pagination, hard-link aliases,
symlink-leaf writes, no-clobber/replace, raw/preview limits, checked-handle races,
scoped jobs and chunk replay. Every fixture is synthetic and temporary.

`python3 tests/files_browser.py`, `python3 tests/files_write_browser.py` and the
file HTTP suites exercise the actual frontend, including error presentation and
mobile layouts. HTTP streaming tests also cover cancellation/backpressure,
reader admission and changed-file transfer errors. Existing native CLI files
and production service directories are never used as mutation fixtures.
