# Files and FileDock

Directory browsing, standalone file previews, downloads, uploads and file-manager
operations are provided by the independent **FileDock** service. Its entry points
are `files.html?node=ID&path=ABS` and `file.html?node=ID&path=ABS` beneath the configured
file-service base URL. A file is identified by its machine and absolute path.

SessionDock's `file.html` and `files.html` are entry adapters. They derive the node
from the selected session's qualified UID (or `/api/meta` for a local node), resolve
a relative conversation reference through `POST /api/session/resolve-files`, and
navigate to FileDock. An explicit `node` and absolute `path` need no conversation
lookup. Failed resolution stays on the adapter page with the actual error and a
refresh action. The destination contains only `node` and `path`.

The default file-service base is `/files/` on the current origin. A deployment may
supply `filedock_url` in the capabilities declaration for a different origin.
The authenticated public proxy routes `/sessiondock/` and `/files/` independently.
FileDock has its own binary, private state, node tokens and deployment lifecycle.

Conversation-specific responsibilities remain here:

- Recognizing file references in message/tool content, resolving relative paths
  against the selected cwd, and disambiguating names.
- Inline conversation media and its checked file responses.
- Uploading/recording conversation and bug-report attachments.
- Serving existing file API consumers during the node/client migration.

FileDock does not load session history, establish conversation grants, or attach
uploads to a conversation. Its filesystem worker retains checked handles, OS
permission checks, bounded previews and streaming. Directory and file-manager
UI preferences belong to FileDock.

Validation: `python3 tests/files_browser.py` exercises actual conversation links,
node/path routing, unresolved-reference errors and direct directory entry on
desktop/mobile. FileDock's own browser and node/hub suites validate previews,
downloads and mutations against synthetic files.
