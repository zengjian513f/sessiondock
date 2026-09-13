"""Free, isolated Chromium acceptance test for the Rust + legacy vertical slice.

Build sessiondock first. Requires Python Playwright and its Chromium (or an
explicit PLAYWRIGHT_CHROMIUM_EXECUTABLE). Never starts a CLI or reads native homes.
"""

import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from urllib.request import urlopen

from playwright.sync_api import sync_playwright, expect


REPO = Path(__file__).resolve().parents[1]
FIXTURES = REPO / "crates/sessiondock/tests/fixtures"


def main():
    executable = REPO / "target/debug" / ("sessiondock.exe" if os.name == "nt" else "sessiondock")
    if not executable.is_file():
        raise SystemExit("Run cargo build -p sessiondock --locked first")
    with tempfile.TemporaryDirectory(prefix="sessiondock-browser-") as temporary:
        data = Path(temporary) / "fixtures"
        shutil.copytree(FIXTURES, data)
        environment = {k: v for k, v in os.environ.items() if not k.startswith("SESSIONDOCK_")}
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        base = f"http://127.0.0.1:{port}"
        environment.update({"SESSIONDOCK_BIND": f"127.0.0.1:{port}",
            "SESSIONDOCK_CLAUDE_ROOT": str(data / "claude"),
            "SESSIONDOCK_CODEX_ROOT": str(data / "codex"),
            "SESSIONDOCK_GROK_ROOT": str(data / "grok")})
        process = subprocess.Popen([str(executable)], cwd=REPO, env=environment,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            for _ in range(100):
                if process.poll() is not None:
                    raise AssertionError(f"Rust startup failed: {process.stderr.read()}")
                try:
                    with urlopen(base + "/api/health", timeout=1) as response:
                        assert response.status == 200
                    break
                except OSError:
                    time.sleep(0.05)
            else:
                raise AssertionError("Rust health check timed out")

            with sync_playwright() as playwright:
                launch = {"headless": True}
                if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                    launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
                browser = playwright.chromium.launch(**launch)
                context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                page = context.new_page()
                errors, requests = [], []
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.on("request", lambda request: requests.append(request.url))
                response = page.goto(base, wait_until="networkidle")
                assert response.status == 200
                expect(page.locator("#backend-notice")).to_be_hidden()  # batch 44: no standing banner
                expect(page.locator("#session-active")).to_have_text("0" if sys.platform.startswith("linux") else "?")
                expect(page.locator("#side .item[data-uid]")).to_have_count(3)
                assert page.evaluate("AgentHubCapabilities.namespace") == "sessiondock."
                assert page.evaluate("T.enabled") is False

                for source, answer in [
                    ("codex", "Codex 人工样例读取正常"),
                    ("grok", "Grok 人工样例读取正常"),
                    ("claude", "Claude 人工样例读取正常"),
                ]:
                    page.locator(f'#side .item[data-uid^="{source}:"]').click()
                    expect(page.locator("#msgs")).to_contain_text(answer)
                    expect(page.locator("#a-term")).to_be_visible()
                    expect(page.locator("#a-term")).to_be_enabled()
                page.wait_for_function("_es && _es.readyState === EventSource.OPEN")
                page.locator("#a-term").hover()
                expect(page.locator("#console-toast")).to_contain_text("只读")
                dialogs = []
                def accept_dialog(dialog):
                    dialogs.append(dialog.message)
                    dialog.accept()
                page.on("dialog", accept_dialog)
                page.locator("#a-term").click()
                assert dialogs and "只读" in dialogs[-1]

                path = data / "claude/project-demo/synthetic-claude.jsonl"
                original = path.read_bytes()
                addition = (json.dumps({"type": "user", "uuid": "browser-user-2",
                    "parentUuid": "claude-duration-1", "sessionId": "synthetic-claude",
                    "timestamp": "2026-09-11T10:00:04.123Z",
                    "message": {"role": "user", "content": "SSE 半行中文追加只出现一次"}},
                    ensure_ascii=False) + "\n").encode()
                # Test writers modify only this invocation's temporary synthetic fixture.
                split = len(addition) // 2
                with path.open("ab") as stream:
                    stream.write(addition[:split])
                page.wait_for_timeout(900)
                expect(page.locator("#msgs")).not_to_contain_text("SSE 半行中文追加只出现一次")
                with path.open("ab") as stream:
                    stream.write(addition[split:])
                expect(page.locator("#msgs")).to_contain_text("SSE 半行中文追加只出现一次")
                assert page.locator("#msgs").inner_text().count("SSE 半行中文追加只出现一次") == 1

                # Unreadable native grammar must not leave a silently stale 'live'
                # transcript. Since batch 33 unknown record kinds are skipped like
                # Python, so the trigger is a scalar `content` (unreadable for both).
                unsupported = json.dumps({"type": "user", "sessionId": "legacy",
                    "message": {"role": "user", "content": 42}}) + "\n"
                with path.open("ab") as stream:
                    stream.write(unsupported.encode())
                expect(page.locator("#migration-read-error")).to_be_visible(timeout=10000)
                expect(page.locator("#migration-read-error")).to_contain_text("历史")
                expect(page.locator("#a-term")).to_be_visible()
                # Repair and explicitly retry; no automatic write/CLI operation is involved.
                path.write_bytes(original + addition)
                page.locator("#migration-read-error button").click()
                expect(page.locator("#migration-read-error")).to_have_count(0)
                page.wait_for_function("_es && _es.readyState === EventSource.OPEN")

                # Truncation invalidates the byte cursor and replaces the old view.
                path.write_bytes(original)
                expect(page.locator("#msgs")).not_to_contain_text("SSE 半行中文追加只出现一次")
                expect(page.locator("#msgs")).to_contain_text("Claude 人工样例读取正常")
                page.set_viewport_size({"width": 375, "height": 812})
                page.emulate_media(color_scheme="dark")
                # Legacy switches to its mobile list page on the first narrow viewport.
                page.locator('#side .item[data-uid^="claude:"]').click()
                expect(page.locator("#a-term")).to_be_visible()
                expect(page.locator("#a-term")).to_be_enabled()
                assert page.evaluate("document.documentElement.scrollWidth <= innerWidth")
                assert not errors, errors
                for suffix in ["/api/audit/browser", "/api/live", "/api/session/outbox", "/api/session/resolve-files"]:
                    assert not any(suffix in url for url in requests), (suffix, requests)

                # Shutdown must finish even while a browser still holds its SSE connection.
                process.terminate()
                process.wait(timeout=5)
                if os.name != "nt":
                    assert process.returncode == 0
                browser.close()
                print("PASS legacy browser: native fixtures, SSE append/partial/reset, explicit errors/retry, console, mobile/dark, shutdown")
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()  # Only the isolated test child created above.
                    process.wait(timeout=5)


if __name__ == "__main__":
    main()
