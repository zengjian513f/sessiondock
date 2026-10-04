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

from frontend_paths import entry_asset, frontend_dir, html_assets, local_asset

ROOT = Path(__file__).resolve().parent.parent

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


def check_frontend(failures):
    web = frontend_dir()
    entry = entry_asset(web)
    main_js = entry.read_text(encoding="utf-8")
    manifest = json.loads((web / "manifest.webmanifest").read_text(encoding="utf-8"))
    for field in ("name", "short_name"):
        if manifest.get(field) != "SessionDock":
            failures.append(f"{web}/manifest.webmanifest: {field} is {manifest.get(field)!r}, expected 'SessionDock'")
    for field in ("id", "start_url", "scope"):
        if manifest.get(field) != "./":
            failures.append(f"{web}/manifest.webmanifest: {field} must keep the relative install scope './'")
    if not manifest.get("icons"):
        failures.append(f"{web}/manifest.webmanifest: missing install icons")
    for icon in manifest.get("icons", []):
        local_asset(web, icon["src"])
    index = (web / "index.html").read_text(encoding="utf-8")
    if '<meta name="apple-mobile-web-app-title" content="SessionDock">' not in index:
        failures.append(f"{web}/index.html: missing SessionDock mobile app title")
    # Legacy spreads installation across pwa.js and the settings bundle; Vue
    # keeps it in the entry bundle. Inspect the actual declared scripts.
    install_js = "\n".join(path.read_text(encoding="utf-8") for path in html_assets(web) if path.suffix == ".js")
    for needle in ("data-pwa-install-button", "beforeinstallprompt", "appinstalled", "安装到桌面"):
        if needle not in install_js:
            failures.append(f"{entry}: missing install flow {needle}")
    for path in sorted(web.rglob("*")):
        if path.is_file() and path.suffix in (".js", ".html"):
            if re.search(r"serviceWorker\s*\.\s*register\s*\(", path.read_text(encoding="utf-8")):
                failures.append(f"{path}: registers a service worker (it pins new tabs to a hung page process)")
    if not re.search(r"startsWith\(['\"]sessiondock-shell-['\"]\)", index):
        failures.append(f"{web}/index.html: does not clear the retired sessiondock-shell-* caches")
    if "getRegistrations" not in index or ".unregister()" not in index or "caches.delete" not in index:
        failures.append(f"{web}/index.html: missing retired service worker/cache cleanup")
    html_assets(web)
    for page, expected in (("files.html", "<title>正在打开文件 · SessionDock</title>"),
                           ("file.html", "<title>正在打开文件 · SessionDock</title>")):
        if expected not in (web / page).read_text(encoding="utf-8"):
            failures.append(f"{web}/{page}: missing {expected}")
        html_assets(web, page)
        if "无法打开文件 · SessionDock" not in entry_asset(web, page).read_text(encoding="utf-8"):
            failures.append(f"{web}/{page}: file adapter missing SessionDock error title")
    if "SessionDock 已更新" not in main_js:
        failures.append(f"{entry}: stale-page notice does not name SessionDock")


def main():
    failures = []
    paths = list(tracked_text_files())
    for path in paths:
        for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if BANNED.search(line):
                failures.append(f"{path.relative_to(ROOT)}:{number}: {line.strip()[:160]}")
    try:
        check_frontend(failures)
    except (AssertionError, OSError, ValueError, KeyError) as exc:
        failures.append(f"built frontend {frontend_dir()}: {exc}; run npm --prefix web run build:migration or select SESSIONDOCK_TEST_WEB_DIR")
    if failures:
        print("FAIL brand names check:")
        for line in failures:
            print("  " + line)
        sys.exit(1)
    print(f"PASS brand names check: {len(paths)} tracked non-Markdown text files contain no retired brand; "
          "manifest/page titles and PWA identity say SessionDock, no service worker is registered")


if __name__ == "__main__":
    main()
