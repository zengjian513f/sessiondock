# Files and FileDock

Directory browsing and file-manager operations are optionally provided by the independent **FileDock** service. Set `SESSIONDOCK_FILEDOCK_URL` on the node or Hub serving the page; unset disables the external integration. Its entry points use `?node=ID&path=ABS` at that configured base. Both files and directories use this entry. A file is
identified by its machine and absolute path; URL-encode query values.

SessionDock's `file.html` and `files.html` are entry adapters. They derive the node
from the selected session's qualified UID (or `/api/meta` for a local node), resolve
a relative conversation reference through `POST /api/session/resolve-files`, and
navigate to FileDock. An explicit `node` and absolute `path` need no conversation
lookup. Failed resolution stays on the adapter page with the actual error and a
refresh action. The destination contains only `node` and `path`.

A referenced bare filename resolves against the selected session's cwd first,
even when history mentions a same-named file in another project. If that path
does not exist (or cwd is unavailable), resolution falls back to recorded paths
and immediate children of recorded directories. Multiple distinct fallback
targets remain ambiguous and require an explicit path; directories are not
searched recursively. Filesystem permission and access errors are preserved.

`capabilities.file_browser` and `filedock_url` advertise the configured integration
in `/api/meta` and the HTML capability declaration, including both adapter pages.
Prefer a same-origin authenticated reverse proxy. FileDock has its own binary,
private state, node tokens and deployment lifecycle. Its node mapping must use
the same stable identity as SessionDock; standalone nodes may configure only
`SESSIONDOCK_NODE_ID_FILE`, without a Hub listener.

With FileDock configured, the session title bar's directory (inline or in the
⋯ menu) is a link that opens `files.html?uid=UID&path=CWD` in a new tab; the
adapter derives the node and navigates to FileDock at that machine and cwd.
This also applies to launch-only sessions before native history exists or a
binding is confirmed: their receipt supplies cwd and the pending UID supplies
the machine. Both header paths reuse the same directory component.
Without FileDock the directory is plain text, not a link.

DELTA: without FileDock, conversation file links retain built-in text/media
previews and downloads via the existing single-file API. Direct path links and
directories explain that directory browsing is not configured and offer copying
the resolved path. They never navigate to an assumed external service.

Conversation-specific responsibilities remain here:

- Recognizing file references in message/tool content, resolving relative paths
  against the selected cwd, and disambiguating names.
- Inline conversation media and its checked file responses.
- Uploading/recording conversation and bug-report attachments, and the
  console file paste that publishes straight into `sessiondock_attachments`
  ([terminal-input.md](terminal-input.md#console-file-paste)).
- Serving one referenced file (`GET /api/session/file`: raw, `mode=info`,
  `mode=preview`, `download=1`) for the conversation's own links.

DELTA: SessionDock's directory listing, file-manager actions and chunked
uploads (`GET /api/session/files`, `POST /api/session/files/action`,
`POST /api/session/files/upload`, the JSON upload-completion form of
`POST /api/session/attachment` and the `files_jobs`/`files_write`
capabilities) were removed on 2026-10-07; FileDock owns them.

Conversation links recognize absolute Windows drive paths (`X:\project\file.md`
and `X:/project`) in code spans, Markdown links and parenthesized prose. Drive
letters are file references rather than URL schemes. Resolution remains scoped
to the selected session and happens on its node; copying a file's parent preserves
the drive-root separator (`X:\` or `X:/`).

Web addresses beginning with `http://`, `https://` or `www.` are clickable in
ordinary prose, including bold and italic text. Emphasis markers and trailing
prose punctuation remain outside the destination. Markdown links retain their
labels, and fenced code remains literal. File references in ordinary prose
still require parentheses, code spans or explicit Markdown links.

FileDock does not load session history, establish conversation grants, or attach
uploads to a conversation. Its filesystem worker retains checked handles, OS
permission checks, bounded previews and streaming. Directory and file-manager
UI preferences belong to FileDock.

Validation: `python3 tests/files_browser.py` exercises actual conversation links,
node/path routing, unresolved-reference errors and direct directory entry on
desktop/mobile. `tests/autobind_browser.py` clicks the launch-only directory
before the fake CLI writes its first native record, in wide/narrow light/dark
views (regression BUG-20261015-153941-18a613). FileDock's own browser and
node/hub suites validate previews,
downloads and mutations against synthetic files.

DELTA (BUG-20261013-223056-f48de9): configured FileDock previews CSV as a table
with a source toggle, reusing its document table styles. CSV parsing and bounded
preview handling belong to FileDock; SessionDock continues to resolve and route
the file reference. Its standalone fallback without FileDock remains plain text.
The report's browser event 226 opened the CSV link in a new page; native history
and the lifecycle binding agreed on the referenced file and session. FileDock's
text renderer classified CSV as source-only, so no table mode was available.
FileDock's `tests/csv_browser.py` now clicks a real SessionDock conversation link
through the resolver and FileDock node/hub, then exercises table/source switching,
copy, download, directory reopening and truncated CSV exports in Chromium.
