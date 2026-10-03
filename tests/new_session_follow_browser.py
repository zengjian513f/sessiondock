#!/usr/bin/env python3
"""A new Claude session follows its launch page to the native history in place.

Claude writes a startup notice (for example a SessionStart hook's message)
before any input; that record creates the native history and the page leaves
the launch receipt for the real session. Typing in the composer while that
happens must not be interrupted: no loading interstitial replaces the page,
the composer never hides, it keeps focus and the draft, and the automatic
switch does not add a browser history entry. Desktop and 390 px, fake CLI.
"""
from browser_runtime import js
import json
import os
import subprocess
import tempfile
from pathlib import Path

from playwright.sync_api import sync_playwright, expect

from history_parity import REPO, BINARY, Corpus, isolated_server
from send_browser import SETTINGS, claude_uid, create_claude, initialize

NOTICE = "agents-md: no CLAUDE.md found; AGENTS.md loaded: /synthetic/AGENTS.md"

# Record every step of the switch that would show as a flash or steal focus.
WATCH = """() => {
  window.followEvents = [];
  const composer = document.querySelector('#composer');
  const input = document.querySelector('#cinput');
  input.addEventListener('blur', () => followEvents.push('blur'));
  new MutationObserver(() => {
    if (composer.classList.contains('hidden')) followEvents.push('composer-hidden');
  }).observe(composer, {attributes: true, attributeFilter: ['class']});
  new MutationObserver(() => {
    if (document.querySelector('#detail .spin')) followEvents.push('spinner');
  }).observe(document.querySelector('#detail'), {childList: true, subtree: true});
  window.followHistory = history.length;
}"""


def follow(page, root, base, work, draft):
    receipt = create_claude(page, base, work, open_terminal=False)
    pending = "tmux:" + receipt["name"]
    assert page.evaluate(js("S.sel", 'runtime.core.state.selection.sel')) == pending
    page.locator("#cinput").click()
    page.locator("#cinput").press_sequentially(draft)
    page.evaluate(WATCH)
    # Still on the launch page: the native history does not exist yet.
    assert page.evaluate(js("S.sel", 'runtime.core.state.selection.sel')) == pending
    (root / "notice").write_text(NOTICE)
    native = claude_uid(root, receipt["declared_sid"])
    page.wait_for_function(js("uid => S.sel === uid", 'uid => runtime.core.state.selection.sel === uid'), arg=native, timeout=20000)
    page.wait_for_function("text => document.querySelector('#msgs')?.textContent.includes(text)",
                           arg=NOTICE, timeout=15000)
    state = page.evaluate("""() => ({events: followEvents, active: document.activeElement?.id,
      value: document.querySelector('#cinput').value, history: history.length - followHistory,
      url: location.href})""")
    assert state["events"] == [], state
    assert state["active"] == "cinput", state
    assert state["value"] == draft, state
    assert state["history"] == 0, state
    expect(page.locator("#composer")).to_be_visible()
    return receipt


def main():
    if os.name != "posix":
        print("SKIP new_session_follow_browser: POSIX required", flush=True)
        return
    with tempfile.TemporaryDirectory(prefix="sessiondock-follow-") as temporary:
        root = Path(temporary).resolve()
        for name in ["host", "work", "work/claude-area", "ledger", "state", "bin", "home", "claude", "codex", "grok"]:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        python = Path(subprocess.check_output(["/bin/sh", "-c", "command -v python3"]).decode().strip()).resolve()
        wrapper = root / "bin/fake-claude"
        wrapper.write_text("#!/bin/sh\nexec %s %s \"$@\"\n" % (python, REPO / "tests/fake_claude_cli.py"))
        wrapper.chmod(0o700)
        configuration = root / "launcher.json"
        configuration.touch(mode=0o600)
        configuration.write_text(json.dumps({"schema": 2, "host_binary": str(REPO / "target/debug/ptyhost"),
            "host_dir": str(root / "host"), "adapters": [], "profiles": [
                {"id": "claude-cli-v1", "source": "claude", "executable": str(wrapper),
                 "args": ["--settings", SETTINGS, "--reply"], "new_args": ["--session-id", "{session_id}"],
                 "resume_args": ["--resume", "{sid}"],
                 "env": {"PATH": "/usr/bin:/bin", "HOME": str(root / "home"), "TERM": "xterm-256color",
                         "LANG": "C.UTF-8", "SESSIONDOCK_TEST_CLAUDE_ROOT": str(root / "claude"),
                         "SESSIONDOCK_TEST_CLAUDE_NOTICE": str(root / "notice")}}]}))
        initialize("--initialize-lifecycle", root / "ledger")
        with sync_playwright() as playwright:
            options = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**options)
            try:
                with isolated_server(corpus, BINARY, host_dir=root / "host", lifecycle_dir=root / "ledger",
                                     launcher_config=configuration, state_dir=root / "state",
                                     file_roots=(root / "work",), file_write_roots=(root / "work",)) as (base, _):
                    receipts = []
                    for width, height, draft in [(1280, 900, "draft typed on desktop"), (390, 844, "phone draft")]:
                        context = browser.new_context(viewport={"width": width, "height": height},
                                                      service_workers="block")
                        context.route("**/*", lambda route: route.continue_()
                                      if route.request.url.startswith(base + "/") else route.abort())
                        page = context.new_page()
                        errors = []
                        page.on("pageerror", lambda error: errors.append(str(error)))
                        page.goto(base, wait_until="networkidle")
                        try:
                            receipts.append(follow(page, root, base, root / "work", draft))
                        finally:
                            for item in receipts:
                                context.request.post(base + "/api/term/kill",
                                                     data={"record_id": item["record_id"],
                                                           "instance_id": item["instance_id"]})
                        assert not errors, errors
                        context.close()
            finally:
                browser.close()
    print("PASS new_session_follow_browser: startup notice switches the launch page to the native session "
          "without a spinner, hidden composer, lost focus/draft or extra history entry (desktop + 390 px)")


if __name__ == "__main__":
    main()
