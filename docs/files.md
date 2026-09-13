# Isolated read-only session files

This is the first, read-only part of M6. It does not upload, rename, delete,
extract archives, generate thumbnails, create jobs, or persist browser grants.
It never discovers HOME or reads a native CLI directory to resolve file names.
Enabling it does not make sessions, terminal input, or file operations writable.

## Legacy contract and intentional boundary change

The Python `files.py` resolver accepts paths recorded in the selected semantic
history, including tool arguments, Markdown links, quoted paths, and line-number
suffixes. A basename must itself be mentioned; it can resolve to a uniquely
mentioned full path or a direct child of a mentioned directory. It never needs a
recursive filesystem search. Python's directory browser subsequently grants
authenticated operators navigation throughout the filesystem, and `file_manager`
persists that browser grant. That production authorization is **not** imported.

Rust instead requires both:

1. A successfully resolved SessionStore view for the exact UID/agent, with the
   complete selected semantic message branch and its cwd.
2. An explicitly configured existing development file root containing the
   target. Neither cwd nor a client-supplied absolute path is a grant.

Directory navigation first resolves a mentioned directory, then remains inside
that directory's configured root. A second configured root is not reachable by
changing `path`. Parent navigation stops at the root (`parent: null`). The
additional `root` field lets the legacy page bound its breadcrumbs. Authorization
is repeated per request; an old browser grant cannot outlive its reference.

Transport compatibility:

| Route | Read-only behavior |
| --- | --- |
| `POST /api/session/resolve-files` | `uid`, optional `agent`, at most 256 `refs`; returns legacy `resolved`, `targets`, `file_browser`, plus explicit per-reference `errors` and `incomplete`. |
| `GET /api/session/file` | Exact mentioned `ref`; default raw response, `mode=info` JSON, `mode=preview` media bytes, or `download=1`. Browser-reader redirects are transport-owned. |
| `GET /api/session/files` | Mentioned directory `ref` plus optional absolute `path`; list pagination/sorting or the same `info`, `preview`, download actions. Always `writable: false`. |
| `HEAD` for byte responses | Full representation headers and no body; ignores Range as required by HTTP. |
| jobs, trash, artifact, thumbnail, mutation modes | Explicit 501, never fabricated empty state or successful mutation. |

The caller must propagate unsupported SessionStore histories, not replace them
with an empty reference set. Automatic link detection may leave failed references
as text; manual opening should show the per-reference error. Jobs polling and
automatic thumbnail requests need separate capability gates.

## Module interface

`FileService::open(Vec<PathBuf>)` accepts 1–16 explicit absolute existing roots,
rejecting filesystem roots, duplicates and overlapping roots. The application
must also reject overlap with native histories, credentials/state, ptyhost or
served assets. Do not canonicalize away user-provided symlinks before passing
the original path to this constructor.

`FileScope { uid, agent, cwd, messages }` must come from a selected SessionStore
view, not from client-provided messages/cwd. `target(scope, ref, navigation)`
returns a nonconstructible `ResolvedTarget`; its public path is display-only.
Use `list`, `describe`, and `read` with that target. Do not reopen its path.

`read` returns `FileResponse { status, headers, body }`. `FileBody` is empty,
bounded bytes, or `CheckedReader`; binary data is never encoded in JSON.
`CheckedReader` implements blocking `Read`, caps each call at 64 KiB, and reports
changes/early EOF as errors. The transport must run this work off the async
reactor, limit concurrent readers, retain their permits until workers exit,
apply backpressure, and stop scheduling reads on disconnect/shutdown. Targets
are single-request, single-consumer handles, not a persistent browser cache.

## Filesystem checks and platform behavior

The implementation opens the explicit volume root once, then every actual
component through directory handles with no-follow operations. It retains these
handles and checks root ancestors, intermediate directories, leaf identity and
opened-file metadata before and after reading. Enumeration uses the already-open
directory, never a second ambient `read_dir(path)` lookup. File opens request
no-follow and nonblocking behavior so a swapped-in FIFO cannot silently become
a blocking ordinary-file read. These operations use the capability filesystem
API documented by [cap-std](https://docs.rs/cap-std/latest/cap_std/fs/struct.Dir.html)
and the [no-follow directory extensions](https://docs.rs/cap-fs-ext/latest/cap_fs_ext/trait.DirExt.html).

Symlinks/reparse points, devices, sockets and pipes are rejected. A regular
file with several hard links is readable (`boundary::ordinary`): Python's
bug-report flow and file manager link attachments into project trees
(`os.link`), so a screenshot such as `agenthub_attachments/<n>/image.png`
routinely has two links, and reading one alias discloses nothing the other
alias would not. The write side keeps the stricter `boundary::unshared`
check (below). Unavailable directory entries are retained with explicit
errors rather than followed or silently removed. Hidden-file display means dot
names, matching the legacy option; it is a display filter, not an authorization
boundary. Existing files can be read-only; no permissions are repaired.

Paths are UTF-8, at most 4,096 bytes per input and 64 parsed components. Relative
references use only the selected cwd. `.` and `..` are normalized lexically
before the root check, never by following a symlink; HOME and URLs are not
expanded. On Unix, Windows drive paths and backslashes are explicitly rejected.
Windows accepts explicit drive-rooted native paths and emits forward-slash wire
paths; UNC/device paths, alternate data streams, ambiguous trailing dots/spaces
and reserved device names are rejected. Cross-platform transcript paths are not
silently remapped.

These checks are not a snapshot filesystem or protection from an administrator
changing mounts, OS ACLs, or writing the same inode while concealing all metadata
changes. Already transmitted bytes cannot be recalled. Windows file identity
uses cap-fs-ext's platform support; its documentation notes a [ReFS identifier
limitation](https://docs.rs/cap-fs-ext/latest/cap_fs_ext/trait.MetadataExt.html).
No `/proc` or Linux-only path fallback is used. Linux tests and Windows type
checking do not establish Windows/macOS runtime acceptance or network-filesystem
guarantees.

## Representation and resource policy

- Listing: at most 10,000 scanned entries, pages of 1–500; overflow is 413, not
  a truncated list claiming an exact total. Directories sort first. Name/type
  comparison uses Unicode lowercase, not Python's full Unicode `casefold`.
- Reference parsing: at most 512 MiB of inspected semantic strings, 250,000
  unique references, 32 nested tool-argument levels and 20,000 path probes per
  request. The largest real sessions observed (a 228 MB Codex rollout, a
  174 MB fork chain) hold about 41 MB of semantic text and 47,000 references
  under the Python `files.references` rules, so both limits keep an order of
  magnitude of headroom; Python has no limit. `resolve-files` and the file
  page still answer 413 `file_reference_budget` beyond them; the media index
  degrades instead ([media.md](media.md#text-discovery-versus-file-authority)).
  Mentioned directories and basename indexes are built once per batch.
- Text info preview: 1 MiB, explicit `truncated`; incomplete final UTF-8 character
  is omitted. Invalid UTF-8 or NUL-containing data is unsupported, not lossy text.
- Default raw opening: at most 32 MiB. HTML, SVG and scripts are plain text,
  never executable origin content. Unknown binary files are attachments.
- Recognized raster image, audio/video and PDF previews stream from the handle.
  A PDF must contain `%PDF-` in its first 1,024 bytes; invalid PDF preview/info
  errors still allow an explicit octet-stream download. No external decoder is
  launched, and image/media decoder validity is left to the browser.
- Explicit download/media stream: at most 16 GiB per file. Download is always
  `application/octet-stream` with safe fallback and UTF-8 filename parameters.
- Exactly one `bytes=start-end`, open-ended, or positive suffix range returns
  206; invalid, multipart, suffix-zero, or unsatisfiable ranges return 416 with
  `Content-Range: bytes */size`. Empty files work without a range. Range handling
  does not require materializing a full streamed download. HEAD ignores Range;
  unknown range units are ignored, following [RFC 9110 section 14.2](https://www.rfc-editor.org/rfc/rfc9110.html#section-14.2).
  Without a matching strong validator, transport must also ignore Range when
  If-Range is present, avoiding assembly of different file versions.

Responses have no-store, nosniff, no-referrer and explicit Content-Disposition.
Non-PDF content uses `sandbox; default-src 'none'`. A validated inline PDF uses
`frame-ancestors 'self'` so Chromium's isolated native viewer can render it.
Reading/preparing MIME for HEAD can still perform bounded validation I/O.

Focused tests are `cargo test -p sessiondock --lib files::tests:: --locked`
and `cargo test -p sessiondock --test files --locked`.
They create only artificial temporary files and cover scope/reference isolation,
navigation limits, symlinks/hardlinks/special files, concurrent replacement,
per-chunk modification, listing budgets, preview policy, HEAD and single ranges.
HTTP tests additionally verify two unpolled bodies retain admission capacity,
dropping them releases it, slow consumers do not prefetch later chunks, changed
files produce body errors instead of successful short EOF, shutdown cancels
transfers, and ordinary read requests leave all native/file bytes unchanged.

## Write operations under explicit write roots (batch 28)

`POST /api/session/files/action`, `POST /api/session/files/upload` and
`POST /api/session/attachment` exist only under
`SESSIONDOCK_FILE_WRITE_ROOTS` (1..16 existing absolute directories, `os.pathsep`
separated, no `..`, not a filesystem root). Every write root must equal or lie
inside a configured read root — read roots never become writable implicitly
and write targets are still resolved through the read-side session references —
and must be disjoint from the web, native, state, host, delivery, lifecycle,
launcher and audit paths and from other write roots. Unset keeps the three
routes 501 `files_jobs_disabled`, `mode=jobs` 501, and `files_jobs:false`;
configured, `files_jobs:true` and `files_write` describes
`{actions, conflicts:["error","keep","skip"], delete:"trash", chunk_bytes,
job_bytes, max_jobs, expiry_seconds, max_items}`.

Every request (including every chunk) carries `uid`, optional `agent` and
`ref` (a directory anchor the session mentions) and is re-resolved through the
SessionStore and `FileService::target`; the target must be inside the anchored
read root and inside a write root; `.`/`..`/relative paths and `.sessiondock-*`
reserved names are 400, the write root itself is immutable (403). A source
entry that is a regular file with more than one hard link is 403
`file_hardlink_forbidden` for rename/move/delete (`boundary::unshared`):
detaching one alias of shared data is never done on the user's behalf, while
reading such a file is ordinary.

- `action` JSON (≤ 512 KiB, `deny_unknown_fields`): `{uid, agent?, ref, action,
  paths?, destination?, name?, conflict?, size?, modified?, sha256?, job?}`.
  `upload` registers a job (`{job:{id, state:"uploading", bytes, total_bytes,
  …}}`, immediate 409 `file_exists` for `conflict:error`, zero size completes
  at once); `mkdir|new-file|rename|move|delete` run synchronously and return
  the job with `completed[]`/`errors[{path,error,code,status}]` (partial
  failure is 200 with state failed; total failure uses the first error's
  status); `cancel` → 200. `delete` is offered (listed in `actions`) only
  with `SESSIONDOCK_STATE_DIR`; without it every item is 501
  `file_trash_unconfigured` and nothing is removed. `copy/compress/extract/bundle/trash/restore/purge/
  retry` → 501 `file_action_not_implemented`; `conflict:replace` → 400.
- `upload?uid&agent&ref&job&offset` with `application/octet-stream`: `offset`
  must equal the bytes received; resending the previous accepted chunk (same
  offset and bytes) is idempotent 200, anything else 409 `file_upload_offset`
  with `expected`; over the declared size 413 `file_upload_overflow`; chunk over
  4 MiB 413; the final chunk publishes synchronously, a sha256 mismatch is 409
  `file_upload_checksum` (staging discarded); publishing onto an existing name
  is 409 `file_exists`; other scope 404 `file_job_unknown`; expired 410
  `file_job_expired`; finished 409 `file_upload_finished`.
- `GET /api/session/files?…&mode=jobs` → `{jobs:[…]}` for the scope (≤ 100).
- `attachment {uid, agent?, ref, job}` records a completed upload's final path;
  with `SESSIONDOCK_STATE_DIR` it is written to metadata (`recorded:true`,
  idempotent, ≤ 256 per session), otherwise `recorded:false`. This differs from
  Python's raw byte-stream attachment contract; the legacy caller sits behind
  the outbox gate and never reaches it in Rust mode.

Atomicity (no unsafe code, no new dependencies): regular files are published by
`linkat` into the target name (an existing name fails with EEXIST and is never
replaced), identity re-checked, then the source name removed; filesystems
that refuse hard links fall back to an `O_EXCL` copy with source-stamp checks.
Directories use `mkdirat`/`O_EXCL`; directory rename/move checks the target
then `renameat` — non-empty directories and files are never replaced, the
only residual race is an empty directory appearing inside the window. Delete
is a `renameat` into `<SESSIONDOCK_STATE_DIR>/file-trash/<32 hex>/<name>`
with a `manifest.json` (`id, path, name, kind, deleted, uid, agent`), the
Rust counterpart of Python's private `file-manager/trash/<uuid>/data`: the
recycle directory never appears inside a project tree (the earlier
`<root>/.agenthub-trash` did, and showed up as untracked repository
content). `file-trash` is the one subdirectory the metadata store tolerates
in the state directory; it is created 0700 at startup and re-verified
(no-follow, ancestry) before every move. A cross-device source is 409
`file_trash_cross_device` and stays in place — nothing is ever unlinked or
copied-then-removed, unlike Python's `shutil.move` fallback. Staging lives in
`<root>/.sessiondock-upload/` (0700, verified not a link) as `<job>.part`
(0600, `create_new`); a stale `.agenthub-upload`/`.agenthub-trash` left by
an earlier build is an ordinary hidden directory now and can be removed. Every operation runs through retained directory handles with
`verify_identity()` before and after (root ancestry, no-follow reopen of each
component, target directory inode); a component swapped for a symlink between
resolve and write is 409 `file_changed` with nothing written outside.

Limits: 8 concurrent uploads (429 `file_jobs_limit`), 256 MiB per job (413),
4 MiB per chunk, 10 minutes of inactivity (410, staging removed), ≤ 256 items
per batch, names ≤ 255 bytes, ≤ 64 finished jobs retained; all adjustable
through `Config.file_write_limits` for tests. Legacy reads `files_write` and
hides unsupported actions (including `delete` when no trash is configured),
removes the "overwrite" conflict choice, uses `chunk_bytes`, and explains that
delete moves into the server's private recycle directory; Python pages are
unchanged. The upload picker falls back to the listed directory when it was
opened without the toolbar button (drop, script), so an action request never
carries an empty `destination`; every failed action, upload chunk, transfer or
task button reports the server's message in the status line. A refused
navigation (outside the bounding root, missing directory) restores the last
listing — crumbs, address, paging, rows — and the URL, and shows the reason;
crumbs above `root` are rendered as labels, not links.

Validation: `cargo test -p sessiondock files:: --locked` (9 write tests
with TOCTOU injection points, hard-link read/write asymmetry, trash without
a state directory), `--test files_write` (4), and
`python3 tests/files_write_browser.py` (real UI chunked upload, jobs panel,
overwrite error, rename, 390 px delete into `<state>/file-trash`, read-only
sibling stays read-only, native bytes unchanged); `python3 tests/files_browser.py`
covers bounded crumbs and refused-navigation rollback. Still 501: thumbnails,
artifact/trash listing modes, copy/compress/extract/bundle/restore/purge/retry.
