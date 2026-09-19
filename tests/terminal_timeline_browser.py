"""Console replay timeline of an ended SSH session (both console renderers).

The session list is the index of recordings: an exited shell row opens its
recording read-only in the console pane, with a timeline underneath. Seeking to
the start shows the screen before any output; play at 16x brings the output back
and ends at the tail (play button back to ▶); seeking to the end shows the final
screen; the pane stays read-only throughout. No page errors.
"""
import json
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from playwright.sync_api import sync_playwright  # noqa: E402

from history_parity import REPO, BINARY, Corpus, isolated_server  # noqa: E402
from draft_sync_browser import SHELL, initialize  # noqa: E402

XTERM_TEXT = """() => [...T.views.values()].map(view => {
  const buffer = view.term?.buffer?.active;
  return buffer ? Array.from({length:buffer.length}, (_,i) =>
    buffer.getLine(i)?.translateToString(true) || '').join('\\n').trimEnd() : '';
}).join('\\n')"""
TIMELINE = "() => { const v = [...T.views.values()].find(v => v.replay); return v ? {...v.timeline} : null; }"


def seek(page, fraction):
    page.evaluate(
        """f => { const s = document.querySelector('#tl-seek'); s.value = String(Math.round(f * 1000));
                  s.dispatchEvent(new Event('input', {bubbles: true}));
                  s.dispatchEvent(new Event('change', {bubbles: true})); }""",
        fraction)


def run(browser, base, root, renderer):
    errors = []
    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
    context.add_init_script(
        "localStorage.setItem('sessiondock.consoleRenderer', JSON.stringify(%s));" % json.dumps(renderer))
    page = context.new_page()
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.goto(base, wait_until="networkidle")
    shell = context.request.post(base + "/api/term/create", data={
        "source": "shell", "cwd": str(root / "work"), "request_id": f"timeline-{renderer}"}).json()
    name = shell["name"]
    uid = "tmux:" + name
    page.evaluate("info => openPendingSession(info)", shell)
    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN", timeout=10000)
    assert not page.evaluate("document.querySelector('#termpane').classList.contains('replay')"), "live view has no timeline"
    kb = page.locator("#termpane .xterm-helper-textarea")
    kb.press_sequentially("hello")
    kb.press("Enter")
    page.wait_for_function("(" + XTERM_TEXT + ")().includes('RS_UNKNOWN')", timeout=10000)
    time.sleep(0.4)  # a recorded gap worth scrubbing over
    kb.press_sequentially("quit")
    kb.press("Enter")
    page.wait_for_function("[...T.views.values()].some(v => v.ended)", timeout=15000)
    # Live exit on this page must switch to recording replay with a timeline.
    # Reloading and clicking the row is a second path; the first mismatch in
    # BUG-20260919-122121-bbc5dc was staying on the clipped live tail.
    page.wait_for_function("document.querySelector('#termpane').classList.contains('replay')"
                           " && getComputedStyle(document.querySelector('#term-timeline')).display === 'flex'",
                           timeout=15000)
    assert page.evaluate("getComputedStyle(document.querySelector('#xterm')).overflow") in ("auto", "scroll")
    assert page.locator("#a-term").get_attribute("data-unavailable") == "false"
    page.locator(f'#side .item[data-uid="{uid}"]').first.click()
    page.wait_for_function("document.querySelector('#termpane').classList.contains('replay')"
                           " && getComputedStyle(document.querySelector('#term-timeline')).display === 'flex'",
                           timeout=15000)
    print(f"PASS {renderer} live-exit (same page switches to replay with a timeline)", flush=True)
    deadline = time.monotonic() + 15
    row = None
    while time.monotonic() < deadline:
        rows = [r for r in context.request.get(base + "/api/term/list?force=1").json().get("pending", [])
                if r["name"] == name]
        if rows and rows[0].get("state") == "exited" and rows[0].get("recording"):
            row = rows[0]
            break
        time.sleep(0.3)
    assert row and row["recording"]["live"] is False, row
    print(f"PASS {renderer} a (exited row listed with recording)", flush=True)

    page.goto(base, wait_until="networkidle")
    page.wait_for_function(f"!!document.querySelector('#side .item[data-uid=\"{uid}\"]')", timeout=15000)
    page.locator(f'#side .item[data-uid="{uid}"]').first.click()
    page.wait_for_function("[...T.views.values()].some(v => v.replay)", timeout=15000)
    page.wait_for_function("(" + XTERM_TEXT + ")().includes('RS_UNKNOWN')", timeout=15000)
    page.wait_for_function("document.querySelector('#termpane').classList.contains('replay')"
                           " && getComputedStyle(document.querySelector('#term-timeline')).display === 'flex'",
                           timeout=5000)
    page.wait_for_function("(" + TIMELINE + ")()?.atEnd === true", timeout=15000)
    tl = page.evaluate(TIMELINE)
    assert tl["end"] >= tl["start"] and tl["clock"] == tl["end"] and tl["live"] is False, tl
    label = page.locator("#tl-time").text_content()
    assert "/" in label and label.split("/")[0].strip() == label.split("/")[1].strip(), label
    assert page.locator("#tl-play").text_content() == "▶"
    assert page.locator("#tl-live").is_hidden()
    # The recorded screen can be taller than the pane; it must not cover the bar.
    assert page.evaluate("(() => { const r = document.querySelector('#tl-play').getBoundingClientRect();"
                         " return document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2)?.id; })()") == "tl-play"
    print(f"PASS {renderer} b (replay opens at the end with a timeline)", flush=True)

    seek(page, 0)
    page.wait_for_function("!(" + XTERM_TEXT + ")().includes('RS_UNKNOWN')", timeout=10000)
    page.wait_for_function("(" + TIMELINE + ")()?.clock <= (" + TIMELINE + ")()?.start + 1000", timeout=5000)
    assert page.locator("#tl-time").text_content().startswith("00:00 /"), page.locator("#tl-time").text_content()
    print(f"PASS {renderer} c (seek to start shows the screen before any output)", flush=True)

    page.select_option("#tl-speed", "16")
    page.locator("#tl-play").click()
    page.wait_for_function("document.querySelector('#tl-play').textContent === '❚❚'", timeout=2000)
    page.wait_for_function("(" + XTERM_TEXT + ")().includes('RS_UNKNOWN')", timeout=20000)
    page.wait_for_function("(" + TIMELINE + ")()?.atEnd === true && document.querySelector('#tl-play').textContent === '▶'",
                           timeout=20000)
    print(f"PASS {renderer} d (play at 16x replays the output and stops at the end)", flush=True)

    seek(page, 0)
    page.wait_for_function("!(" + XTERM_TEXT + ")().includes('RS_UNKNOWN')", timeout=10000)
    seek(page, 1)
    page.wait_for_function("(" + XTERM_TEXT + ")().includes('RS_UNKNOWN')", timeout=10000)
    before = page.evaluate(XTERM_TEXT)
    page.locator("#termpane .xterm-helper-textarea").press_sequentially("zzz")
    page.keyboard.press("Enter")
    time.sleep(0.6)
    assert page.evaluate(XTERM_TEXT) == before, "replay must stay read-only"
    assert page.evaluate("T.ws?.readyState") == 1, "socket stays open for scrubbing"
    print(f"PASS {renderer} e (seek to end shows the final screen; read-only)", flush=True)

    # Leaving the replay hides the timeline again.
    page.evaluate("closeTermPane()")
    page.wait_for_function("document.querySelector('#termpane').classList.contains('hidden')", timeout=5000)
    assert not errors, errors
    context.close()


def main():
    root = Path(tempfile.mkdtemp(prefix="timeline-", dir="/tmp/claude-1000" if Path("/tmp/claude-1000").is_dir() else None))
    for name in ["host", "work", "ledger", "delivery", "state", "bin", "home", "claude", "codex", "grok"]:
        (root / name).mkdir(mode=0o700)
    corpus = Corpus(root)
    cfg = root / "launcher.json"
    cfg.touch(mode=0o600)
    cfg.write_text(json.dumps({
        "schema": 2, "host_binary": str(REPO / "target/debug/ptyhost"), "host_dir": str(root / "host"),
        "adapters": [{"id": "synthetic-shell-v1", "source": "shell", "executable": str(Path("/bin/sh").resolve()),
                      "args": ["-c", SHELL], "env": {"PATH": "/usr/bin:/bin", "TERM": "xterm-256color"}}],
        "profiles": []}))
    initialize("--initialize-lifecycle", root / "ledger")
    initialize("--initialize-delivery", root / "delivery")
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        with isolated_server(corpus, BINARY, host_dir=root / "host", lifecycle_dir=root / "ledger",
                             launcher_config=cfg, delivery_dir=root / "delivery", state_dir=root / "state") as (base, _):
            for renderer in ("xterm", "grid"):
                run(browser, base, root, renderer)
        browser.close()
    print("PASS terminal_timeline_browser", flush=True)


if __name__ == "__main__":
    main()
