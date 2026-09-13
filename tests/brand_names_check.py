#!/usr/bin/env python3
"""No `AgentHub` brand residue in the served frontend (batch 44 WP-F, watcher F9/F10).

Static check over the `legacy-web/` shell files (`index.html`, `files.html`,
`file.html`, `manifest.webmanifest`, `service-worker.js`, `pwa-install.js`):
the installable identity (manifest name/short_name, `apple-mobile-web-app-title`,
the install script's `data-app-name`), the service-worker cache name and
offline text, every page `<title>` and the titles `files.js` / `file.js` set at
runtime must say SessionDock.
Every other spelling of the old name is on the contract whitelist documented in
`docs/glossary.md` ("Names") — wire headers, template markers, meta tags, JS
globals, font family names, the Python namespaces used as one-time migration
sources — and is listed here explicitly so that a new stray brand string
fails the check instead of hiding behind a broad exemption.

Exit 1 with every offending file:line; exit 0 with a summary otherwise.
"""
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WEB = ROOT / "legacy-web"

# Exact contract identifiers (docs/glossary.md "Names"): removed before the scan.
WHITELIST = [
    "AgentHubCapabilities", "AgentHubTypography", "AgentHubFilePreview",
    "agenthub-capabilities", "agenthub-mode", "agenthub-build", "agenthub-highlight-ready",
    "__AGENTHUB_MODE__", "__AGENTHUB_HOSTNAME__", "__AGENTHUB_ASSET_VERSION__",
    "AGENTHUB_CLIS", "agenthubCli", "agenthubHighlight", "__agenthubConnectionId", "__agenthubPageId",
    "X-AgentHub-Protocol", "X-AgentHub-Node-Token", "X-AgentHub-Page", "X-AgentHub-Trace",
    "X-AgentHub-Build", "X-AgentHub-Decoded-Length",
    "AgentHub CJK Sans", "AgentHub CJK Mono Grid", "AgentHub Ubuntu Sans Mono", "AgentHub Cascadia Mono",
    "application/x-agenthub-files",
    # One-time migration sources (docs/migration.md): read, copied forward, never written.
    "agenthub.hub.", "'agenthub.'", "agenthub-files-", "agenthub-shell-",
    # Python-side names quoted in comments/notices.
    "agenthub_attachments", "/agenthub/", "AGENTHUB_TEST_", "AGENTHUB_SESSION",
]
# The installable identity: every page, the manifest, the service worker and the
# install script are scanned line by line (comments excluded).
SCANNED = ["index.html", "files.html", "file.html", "manifest.webmanifest", "service-worker.js", "pwa-install.js"]
COMMENT = re.compile(r"^\s*(?://|<!--|\*|/\*)")


def stripped(text):
    for token in WHITELIST:
        text = text.replace(token, "")
    return text


def main():
    failures = []
    scanned = 0
    for name in SCANNED:
        path = WEB / name
        scanned += 1
        for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if COMMENT.match(line):
                continue
            if re.search(r"agenthub", stripped(line), re.IGNORECASE):
                failures.append(f"{path.relative_to(ROOT)}:{number}: {line.strip()[:160]}")

    manifest = json.loads((WEB / "manifest.webmanifest").read_text(encoding="utf-8"))
    for field in ("name", "short_name"):
        if manifest.get(field) != "SessionDock":
            failures.append(f"legacy-web/manifest.webmanifest: {field} is {manifest.get(field)!r}, expected 'SessionDock'")
    index = (WEB / "index.html").read_text(encoding="utf-8")
    for needle in ('<meta name="apple-mobile-web-app-title" content="SessionDock">',
                   'data-app-name="SessionDock"', 'data-storage-key="sessiondock.pwa-install-dismissed"'):
        if needle not in index:
            failures.append(f"legacy-web/index.html: missing {needle}")
    worker = (WEB / "service-worker.js").read_text(encoding="utf-8")
    if "const CACHE_PREFIX = 'sessiondock-shell-';" not in worker:
        failures.append("legacy-web/service-worker.js: CACHE_PREFIX is not 'sessiondock-shell-'")
    if "SessionDock 当前离线" not in worker:
        failures.append("legacy-web/service-worker.js: offline text does not name SessionDock")
    for page, expected in (("files.html", "<title>文件管理 · SessionDock</title>"),
                           ("file.html", "<title>正在打开 · SessionDock</title>")):
        if expected not in (WEB / page).read_text(encoding="utf-8"):
            failures.append(f"legacy-web/{page}: missing {expected}")
    for script, needles in (("files.js", ["' · 文件管理 · SessionDock'"]),
                            ("file.js", ["' · SessionDock'", "'无法打开文件 · SessionDock'"])):
        text = (WEB / script).read_text(encoding="utf-8")
        for needle in needles:
            if needle not in text:
                failures.append(f"legacy-web/{script}: missing runtime title {needle}")
    # Storage keys: files pages use `<namespace>files-*`, never `<namespace>agenthub-files-*`.
    files_js = (WEB / "files.js").read_text(encoding="utf-8")
    if "namespace + 'files-'" not in files_js or re.search(r"(?:store|restore)\('agenthub-files", files_js):
        failures.append("legacy-web/files.js: preferences are not keyed as <namespace>files-*")

    if failures:
        print("FAIL brand names check:")
        for line in failures:
            print("  " + line)
        sys.exit(1)
    print(f"PASS brand names check: {scanned} legacy-web shell files scanned; manifest/service worker/page titles/"
          f"PWA identity say SessionDock; the remaining `agenthub` spellings are the {len(WHITELIST)} "
          "whitelisted contract identifiers from docs/glossary.md")


if __name__ == "__main__":
    main()
