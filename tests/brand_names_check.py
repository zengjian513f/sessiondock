#!/usr/bin/env python3
"""No `SessionDock` brand residue in the served frontend (batch 44 WP-F, watcher F9/F10).

Recursive static check over every HTML, JavaScript, CSS and manifest under
`legacy-web/`, including generated notices/tooltips and nested credits pages:
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
    "SessionDockCapabilities", "SessionDockTypography", "SessionDockFilePreview", "SessionDockCli", "sessiondockLanguageForPath",
    "sessiondock-capabilities", "sessiondock-mode", "sessiondock-build", "sessiondock-highlight-ready",
    "__SESSIONDOCK_MODE__", "__SESSIONDOCK_HOSTNAME__", "__SESSIONDOCK_ASSET_VERSION__",
    "SESSIONDOCK_CLIS", "sessiondockCli", "sessiondockHighlight", "__sessiondockConnectionId", "__sessiondockPageId",
    "X-SessionDock-Protocol", "X-SessionDock-Node-Token", "X-SessionDock-Page", "X-SessionDock-Trace",
    "X-SessionDock-Build", "X-SessionDock-Decoded-Length",
    "SessionDock CJK Sans", "SessionDock CJK Mono Grid", "SessionDock Ubuntu Sans Mono", "SessionDock Cascadia Mono",
    "application/x-sessiondock-files",
    # One-time migration sources (docs/migration.md): read, copied forward, never written.
    "sessiondock.hub.", "'sessiondock.'", "sessiondock-files-", "sessiondock-shell-",
    # Python-side names quoted in comments/notices.
    "/sessiondock/", "SESSIONDOCK_TEST_", "SESSIONDOCK_SESSION",
    # Exact Python terminal identifiers used for interoperability, not UI prose.
    "tmux:sessiondock-", "`sessiondock-${session.source}", "=== 'sessiondock'",
]
# Scan the entire served textual frontend, including JS-generated tooltips,
# nested credits pages and CSS; a shell-only scan misses ordinary UI copy.
SCANNED = sorted(path.relative_to(WEB).as_posix() for path in WEB.rglob("*")
                 if path.suffix in {".html", ".js", ".css", ".webmanifest"})
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
            if re.search(r"agent\s*hub", stripped(line), re.IGNORECASE):
                failures.append(f"{path.relative_to(ROOT)}:{number}: {line.strip()[:160]}")

    extra_sources = ["crates/ptyhost/src/main.rs", "crates/ptyhost/Cargo.toml",
                     "crates/ptyhost-client/Cargo.toml", "web/index.html"]
    extra_sources.extend(path.relative_to(ROOT).as_posix()
                         for path in sorted((ROOT / "web/src").rglob("*"))
                         if path.suffix in {".vue", ".ts", ".js", ".css", ".html", ".json"})
    for name in extra_sources:
        for number, line in enumerate((ROOT / name).read_text(encoding="utf-8").splitlines(), 1):
            if COMMENT.match(line):
                continue
            if re.search(r"agent\s*hub", stripped(line), re.IGNORECASE):
                failures.append(f"{name}:{number}: {line.strip()[:160]}")

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
    if 'SessionDock 已更新。当前页面已停止发送，请重新加载。' not in (WEB / "app.js").read_text(encoding="utf-8"):
        failures.append("legacy-web/app.js: stale-page notice does not name SessionDock")
    # Storage keys: files pages use `<namespace>files-*`, never `<namespace>sessiondock-files-*`.
    files_js = (WEB / "files.js").read_text(encoding="utf-8")
    if "namespace + 'files-'" not in files_js or re.search(r"(?:store|restore)\('sessiondock-files", files_js):
        failures.append("legacy-web/files.js: preferences are not keyed as <namespace>files-*")

    if failures:
        print("FAIL brand names check:")
        for line in failures:
            print("  " + line)
        sys.exit(1)
    print(f"PASS brand names check: {scanned} legacy-web text files scanned; manifest/service worker/page titles/"
          f"PWA identity and {len(extra_sources)} additional source files say SessionDock; the remaining `sessiondock` spellings are the {len(WHITELIST)} "
          "whitelisted contract identifiers from docs/glossary.md")


if __name__ == "__main__":
    main()
