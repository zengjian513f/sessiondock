"""Free, isolated Chromium acceptance test for the Rust + legacy vertical slice.

Build sessiondock first. Requires Python Playwright and its Chromium (or an
explicit PLAYWRIGHT_CHROMIUM_EXECUTABLE). Never starts a CLI or reads native homes.
"""

from browser_runtime import js
import json
import os
import re
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from urllib.request import urlopen

from playwright.sync_api import sync_playwright, expect
from popups import on_popup  # noqa: E402
from frontend_paths import frontend_dir


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
            "SESSIONDOCK_WEB_DIR": str(frontend_dir()),
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
                expect(page.locator("#backend-notice")).to_be_hidden()  # no standing banner
                expect(page.locator("#session-active")).to_have_text("0" if sys.platform.startswith("linux") else "?")
                expect(page.locator("#side .item[data-uid]")).to_have_count(3)
                assert page.evaluate(js("SessionDockCapabilities.namespace", 'runtime.capabilities.namespace')) == "sessiondock."
                assert page.evaluate(js("T.enabled", 'runtime.terminal.state.enabled')) is False

                for source, answer in [
                    ("codex", "Codex 人工样例读取正常"),
                    ("grok", "Grok 人工样例读取正常"),
                    ("claude", "Claude 人工样例读取正常"),
                ]:
                    page.locator(f'#side .item[data-uid^="{source}:"]').click()
                    expect(page.locator("#msgs")).to_contain_text(answer)
                    expect(page.locator("#a-term")).to_be_visible()
                    expect(page.locator("#a-term")).to_be_enabled()

                # A Codex side thread can exist only in the live TUI while this
                # conversation remains bound to its main native record. The UI
                # must explain that split, then clear the explanation as soon as
                # the TUI reports main again. This injects view state only; no CLI
                # or production terminal is opened by the isolated fixture.
                codex_row = page.locator('#side .item[data-uid^="codex:"]')
                codex_uid = codex_row.get_attribute("data-uid")
                codex_row.click()
                page.evaluate(js("""uid => {
                  T.views.set('test-side-thread', {
                    bindingUid: uid, codexSideThread: true, ended: false, retired: false,
                  });
                  renderConversationTail(cache.get(viewKey(uid))?.activity || null, uid);
                }""", """uid => {
                  runtime.terminal.state.views.set('test-side-thread', {
                    bindingUid: uid, codexSideThread: true, ended: false, retired: false,
                  });
                  runtime.conversationRenderer.renderConversationTail(runtime.core.cache.cache.get(runtime.viewKey(uid))?.activity || null, uid);
                }"""), codex_uid)
                expect(page.locator(".terminal-thread-notice")).to_contain_text(
                    "终端当前位于 Codex side thread")
                expect(page.locator(".terminal-thread-notice")).to_contain_text("Ctrl+/")
                expect(page.locator(".terminal-thread-notice button")).to_have_text("查看 side thread")
                page.evaluate(js("""uid => {
                  T.views.get('test-side-thread').codexSideThread = false;
                  renderConversationTail(cache.get(viewKey(uid))?.activity || null, uid);
                  T.views.delete('test-side-thread');
                }""", """uid => {
                  runtime.terminal.state.views.get('test-side-thread').codexSideThread = false;
                  runtime.conversationRenderer.renderConversationTail(runtime.core.cache.cache.get(runtime.viewKey(uid))?.activity || null, uid);
                  runtime.terminal.state.views.delete('test-side-thread');
                }"""), codex_uid)
                expect(page.locator(".terminal-thread-notice")).to_have_count(0)
                page.locator('#side .item[data-uid^="claude:"]').click()
                page.wait_for_function(js("_es && _es.readyState === EventSource.OPEN", 'runtime.core.sync.watching && runtime.core.sync.watching.readyState === EventSource.OPEN'))
                page.locator("#a-term").hover()
                expect(page.locator("#console-toast")).to_contain_text("只读")
                dialogs = []
                def accept_dialog(dialog):
                    dialogs.append(dialog.message)
                    dialog.accept()
                on_popup(page, accept_dialog)
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
                # transcript. Unknown record kinds are skipped,
                # so the trigger is a scalar `content` (unreadable).
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
                page.wait_for_function(js("_es && _es.readyState === EventSource.OPEN", 'runtime.core.sync.watching && runtime.core.sync.watching.readyState === EventSource.OPEN'))

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

                # Reloading on the mobile conversation page starts there, both
                # from its ?sid= address and from the saved page without one:
                # while the session list is still loading no frame may show it.
                held = []
                sessions_api = re.compile(r"/api/sessions(\?|$)")
                page.add_init_script("""(() => {
                  window.__listFrames = [];
                  const tick = () => {
                    const left = document.querySelector('#left');
                    if (left) window.__listFrames.push(getComputedStyle(left).display !== 'none');
                    requestAnimationFrame(tick);
                  };
                  requestAnimationFrame(tick);
                })()""")
                assert "sid=" in page.url, page.url
                for address in [page.url, base + "/"]:
                    page.route(sessions_api, lambda route: held.append(route))
                    page.goto(address, wait_until="domcontentloaded")
                    for _ in range(100):
                        if held:
                            break
                        page.wait_for_timeout(50)
                    assert held, "session list request was not issued"
                    page.wait_for_timeout(400)
                    expect(page.locator("#left")).to_be_hidden()
                    expect(page.locator("#detail .spin")).to_be_visible()
                    held.pop().continue_()
                    page.unroute(sessions_api)
                    expect(page.locator("#msgs")).to_contain_text("Claude 人工样例读取正常")
                    expect(page.locator("#a-term")).to_be_visible()
                    frames = page.evaluate("window.__listFrames")
                    assert frames and not any(frames), (address, frames)

                # A saved conversation that no longer exists falls back to the list.
                page.evaluate(js("store.set('sel', 'claude:missing-session')", "runtime.core.preferences.set('sel', 'claude:missing-session')"))
                page.goto(base + "/", wait_until="domcontentloaded")
                expect(page.locator("#side .item[data-uid]")).to_have_count(3)
                expect(page.locator("#left")).to_be_visible()
                expect(page.locator("#detail .empty")).to_have_text("从左侧选择一个会话")
                assert page.evaluate("document.body.classList.contains('mobile-detail')") is False
                assert not errors, errors
                # Linux advertises native liveness and therefore requests
                # `/api/live`; the other optional services remain disabled.
                if sys.platform.startswith("linux"):
                    assert any("/api/live" in url for url in requests), requests
                for suffix in ["/api/audit/browser", "/api/session/outbox", "/api/session/resolve-files"]:
                    assert not any(suffix in url for url in requests), (suffix, requests)

                # Shutdown must finish even while a browser still holds its SSE connection.
                process.terminate()
                process.wait(timeout=5)
                if os.name != "nt":
                    assert process.returncode == 0
                browser.close()
                print("PASS legacy browser: native fixtures, Codex side-thread notice, SSE append/partial/reset, explicit errors/retry, console, mobile/dark, mobile reload, shutdown")
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
