# Files and FileDock

Directory browsing, standalone file previews, downloads, uploads and file-manager
operations are provided by the independent **FileDock** service. Its entry points
use `?node=ID&path=ABS` at the configured file-service base URL, for example
`/files/?node=ID&path=ABS`. Both files and directories use this entry. A file is
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

The default file-service base is `/files/` on the current origin. A deployment may
supply `filedock_url` in the capabilities declaration for a different origin.
The authenticated public proxy routes `/sessiondock/` and `/files/` independently.
FileDock has its own binary, private state, node tokens and deployment lifecycle.

Conversation-specific responsibilities remain here:

- Recognizing file references in message/tool content, resolving relative paths
  against the selected cwd, and disambiguating names.
- Inline conversation media and its checked file responses.
- Uploading/recording conversation and bug-report attachments, and the
  console file paste that publishes straight into `sessiondock_attachments`
  ([terminal-input.md](terminal-input.md#console-file-paste)).
- Serving existing file API consumers during the node/client migration.

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
desktop/mobile. FileDock's own browser and node/hub suites validate previews,
downloads and mutations against synthetic files.
