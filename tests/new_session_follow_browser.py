#!/usr/bin/env python3
"""A new Claude session follows its launch page to the native history in place.

Claude writes a startup notice (for example a SessionStart hook's message)
before any input; that record creates the native history and the page leaves
the launch receipt for the real session. Typing in the composer while that
happens must not be interrupted: no loading interstitial replaces the page,
the composer never hides, it keeps focus and the draft, and the automatic
switch does not add a browser history entry. First SEND also follows through
a deliberately delayed history response without mixing native and launch
header actions or showing a global download/render overlay. Desktop and
390/472 px (including keyboard height), light/dark, fake CLI.
"""

import argparse
import json
import os
import subprocess
import tempfile
from pathlib import Path

from playwright.sync_api import sync_playwright, expect

from history_fixtures import REPO, BINARY, Corpus, isolated_server
from private_hosts import private_hosts
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
  new MutationObserver(() => {
    if (document.querySelector('#prog').classList.contains('on')) followEvents.push('progress');
  }).observe(document.querySelector('#prog'), {attributes: true, attributeFilter: ['class']});
  window.followHistory = history.length;
}"""

HEADER = """() => {
  const head = document.querySelector('#detail > .dhead');
  return {title: head.querySelector('h2').textContent,
    height: head.getBoundingClientRect().height,
    buttons: [...head.querySelectorAll('.dhead-actions button')].filter(b => b.offsetWidth)
      .map(b => [b.id || b.dataset.reportBug, b.ariaLabel,
        Math.round(b.getBoundingClientRect().x)])};
}"""


def follow(page, root, base, work, draft, *, send=False, screenshots=None, label='follow'):
    (root / "notice").unlink(missing_ok=True)
    receipt = create_claude(page, base, work, open_terminal=False)
    pending = "tmux:" + receipt["name"]
    assert page.evaluate("S.sel") == pending
    page.locator("#cinput").click()
    page.locator("#cinput").press_sequentially(draft)
    # Hold the actual first-history request across term/list refreshes, as on
    # the reported phone. The UI must keep the launch header until it can
    # replace it once, not write native actions into that older header.
    held = []
    def hold(route):
        held.append(route)
    page.route('**/api/messages/**', hold, times=1)
    before = page.evaluate(HEADER)
    if send:
        expect(page.locator('#csend')).to_be_enabled(timeout=15000)
        with page.expect_response(lambda r: '/api/session/conversation/send' in r.url) as sent:
            page.locator('#csend').click()
        assert sent.value.ok and sent.value.json()['state'] == 'sent', sent.value.text()
        expect(page.locator('#cinput')).to_have_value('')
        page.locator('#cinput').click()
        page.locator('#cinput').fill('后续草稿 preserved while the first reply loads')
        draft = '后续草稿 preserved while the first reply loads'
    page.evaluate(WATCH)
    if not send:
        assert page.evaluate("S.sel") == pending
        (root / "notice").write_text(NOTICE)
    native = claude_uid(root, receipt["declared_sid"])
    page.wait_for_function("uid => S.sel === uid", arg=native, timeout=20000)
    page.wait_for_timeout(1800)  # Keep the response pending through another live/list refresh.
    assert len(held) == 1, len(held)
    if screenshots:
        page.screenshot(path=str(screenshots / f'{label}-loading.png'))
    during = page.evaluate(HEADER)
    assert during == before, {'before': before, 'during': during}
    expect(page.locator('#prog .txt')).to_be_hidden()
    held.pop().continue_()
    expected = 'OK: 执行 /synthetic/prompts/fast_seed_to_endpoint.md 首条消息' if send else NOTICE
    page.wait_for_function("text => document.querySelector('#msgs')?.textContent.includes(text)",
                           arg=expected, timeout=15000)
    state = page.evaluate("""() => ({events: followEvents, active: document.activeElement?.id,
      value: document.querySelector('#cinput').value, history: history.length - followHistory,
      url: location.href})""")
    assert state["events"] == [], state
    assert state["active"] == "cinput", state
    assert state["value"] == draft, state
    assert state["history"] == 0, state
    expect(page.locator("#composer")).to_be_visible()
    # The native view must settle after metadata/live refreshes too.
    settled = page.evaluate(HEADER)
    page.wait_for_timeout(1800)
    assert page.evaluate(HEADER) == settled
    if screenshots:
        page.screenshot(path=str(screenshots / f'{label}-loaded.png'))
    page.locator('#a-more').click()
    expect(page.locator('#session-actions-menu #a-session-action')).to_be_visible()
    page.keyboard.press('Escape')
    page.locator('#a-term').click()
    expect(page.locator('#termpane')).to_be_visible()
    page.wait_for_function('T.ws?.readyState === WebSocket.OPEN')
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    if os.name != "posix":
        print("SKIP new_session_follow_browser: POSIX required", flush=True)
        return
    with tempfile.TemporaryDirectory(prefix="sessiondock-follow-") as temporary, private_hosts(Path(temporary)):
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
                    cases = [(1280, 900, 'light', False), (390, 844, 'dark', False),
                             (1280, 900, 'dark', True), (390, 844, 'light', True),
                             (472, 287, 'light', True), (472, 287, 'dark', True)]
                    for width, height, theme, send in cases:
                        print(f'follow {width}x{height} {theme} send={send}', flush=True)
                        context = browser.new_context(viewport={"width": width, "height": height},
                                                      color_scheme=theme,
                                                      service_workers="block")
                        context.add_init_script("Object.defineProperty(navigator, 'standalone', {value: true})")
                        context.route("**/*", lambda route: route.continue_()
                                      if route.request.url.startswith(base + "/") else route.abort())
                        page = context.new_page()
                        errors = []
                        page.on("pageerror", lambda error: errors.append(str(error)))
                        page.goto(base, wait_until="networkidle")
                        from session_start_fixtures import choose_mode
                        choose_mode(page, 'claude', 'cli')
                        page.locator('#settings-dialog .modal-close').click()
                        try:
                            receipts.append(follow(page, root, base, root / "work",
                                '执行 /synthetic/prompts/fast_seed_to_endpoint.md 首条消息',
                                send=send, screenshots=args.screenshots,
                                label=f'{width}-{theme}-{"send" if send else "notice"}'))
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
          "and first SEND follow without intermediate header changes, progress overlay, spinner, "
          "hidden composer, lost focus/draft or extra history entry; menus and terminal work "
          "(desktop, 390 px, keyboard-height 472 px; light + dark)")


if __name__ == "__main__":
    main()
