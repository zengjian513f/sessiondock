#!/usr/bin/env python3
"""Require SessionDock identity and reject the retired predecessor brand.

The residue scan covers every tracked non-Markdown text file, including source,
tests, configuration, static assets and the frozen frontend reference. Markdown
is intentionally excluded because it may contain historical descriptions.

Exit 1 with every offending file:line; exit 0 with a summary otherwise.
"""
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WEB = ROOT / "legacy-web"

BANNED = re.compile("agent" + r"[\s_.-]*" + "hub", re.IGNORECASE)


def tracked_text_files():
    raw = subprocess.check_output(["git", "ls-files", "-z"], cwd=ROOT)
    for item in raw.split(b"\0"):
        if not item:
            continue
        path = ROOT / item.decode()
        if path.suffix.lower() == ".md":
            continue
        try:
            path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        yield path


def main():
    failures = []
    paths = list(tracked_text_files())
    for path in paths:
        for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if BANNED.search(line):
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
    if "serviceWorker.register(" in index:
        failures.append("legacy-web/index.html: registers a service worker (it pins new tabs to a hung page process)")
    if "key.startsWith('sessiondock-shell-')" not in index:
        failures.append("legacy-web/index.html: does not clear the retired sessiondock-shell-* caches")
    for page, expected in (("files.html", "<title>正在打开文件 · SessionDock</title>"),
                           ("file.html", "<title>正在打开文件 · SessionDock</title>")):
        if expected not in (WEB / page).read_text(encoding="utf-8"):
            failures.append(f"legacy-web/{page}: missing {expected}")
    if "'无法打开文件 · SessionDock'" not in (WEB / "file.js").read_text(encoding="utf-8"):
        failures.append("file adapter missing SessionDock error title")
    if 'SessionDock 已更新。' not in (WEB / "app.js").read_text(encoding="utf-8"):
        failures.append("legacy-web/app.js: stale-page notice does not name SessionDock")
    if failures:
        print("FAIL brand names check:")
        for line in failures:
            print("  " + line)
        sys.exit(1)
    print(f"PASS brand names check: {len(paths)} tracked non-Markdown text files contain no retired brand; "
          "manifest/page titles and PWA identity say SessionDock, no service worker is registered")


if __name__ == "__main__":
    main()
