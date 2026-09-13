#!/usr/bin/env python3
"""Three-state /api/live with the legacy UI: a managed instance is running, then
exited; unrelated sessions stay explicitly unknown. Only a synthetic free shell."""
import os
from pathlib import Path
import tempfile
import time
import uuid
from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, Corpus, claude_row, codex_row, codex_message, get_json, isolated_server
from host_identity import host


def wait_live(opener, base, predicate, seconds=10):
    deadline = time.monotonic() + seconds
    while True:
        live = get_json(opener, base, "/api/live?force=1")
        if predicate(live):
            return live
        assert time.monotonic() < deadline, live
        time.sleep(0.2)


def main():
    if os.name != "posix":
        raise SystemExit("This isolated real-shell acceptance currently requires POSIX.")
    with tempfile.TemporaryDirectory(prefix="sessiondock-live-ui-") as temporary:
        root = Path(temporary)
        for name in ["host", "work", "claude", "codex", "grok"]:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        sid = "synthetic-native-sid"
        corpus.put(sid, "codex", [codex_row("session_meta", {"id": sid, "cwd": str(root / "work")}),
                                  codex_message("user", "Synthetic live managed console")], [])
        corpus.put("synthetic-unrelated-codex", "codex", [
            codex_row("session_meta", {"id": "synthetic-unrelated-codex", "cwd": str(root / "work")}),
            codex_message("user", "Synthetic unrelated codex session")], [])
        corpus.put("synthetic-unrelated-claude", "claude", [
            claude_row("synthetic-unrelated-claude", "user", "u0", None, "Synthetic unrelated claude question"),
            claude_row("synthetic-unrelated-claude", "assistant", "a0", "u0", "Synthetic unrelated claude answer")], [])
        uid = corpus.uid(sid)
        others = [corpus.uid("synthetic-unrelated-codex"), corpus.uid("synthetic-unrelated-claude")]
        native = {name: path.read_bytes() for name, path in corpus.paths.items()}
        instance = "synthetic-" + uuid.uuid4().hex
        with host(root, instance, uid=uid) as (process, record), sync_playwright() as playwright:
            launch = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**launch)
            try:
                with isolated_server(corpus, BINARY, host_dir=root / "host") as (base, opener):
                    # API: the managed instance is running with a verified identity;
                    # unrelated sessions are not listed and therefore unknown.
                    live = wait_live(opener, base, lambda d: d["uids"] == [uid])
                    assert live["enabled"] is True and live["known"] is True and live["partial"] is True, live
                    assert live["tmux_uids"] == []
                    assert live["started_at"][uid] > 1e9
                    session = live["managed"]["sessions"][uid]
                    assert session["state"] == "running" and session["evidence"] == "host_info", session
                    assert session["pid"] == record["pid"] and session["instance_id"] == instance, session
                    assert session["started_at"] == live["started_at"][uid]
                    assert all(other not in live["managed"]["sessions"] for other in others), live["managed"]["sessions"]
                    assert live["managed"]["unlisted"] == {"state": "unknown", "reason": "no_instance"}
                    assert live["managed"]["process_identity"] == "linux_proc"
                    assert live["managed"]["hosts"][0]["process"]["status"] == "verified"
                    assert live["managed"]["hosts"][0]["process"]["host"]["pid"] == process.pid
                    cached = get_json(opener, base, "/api/live")
                    assert cached["managed"]["cache"]["hit"] is True and cached["uids"] == [uid]
                    encoded = str(live)
                    assert record["sock"] not in encoded and "token" not in live["managed"]["hosts"][0]["summary"]

                    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                    context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
                    requests = []
                    context.on("request", lambda request: requests.append(request.url))
                    errors = []
                    context.on("page", lambda page: page.on("pageerror", lambda error: errors.append(str(error))))
                    page = context.new_page()
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(base, wait_until="networkidle")

                    def unrelated_stay_unknown():
                        # The legacy header cannot claim a global running count
                        # and the active-only filter refuses to hide sessions.
                        expect(page.locator("#session-active")).to_have_text("?")
                        expect(page.locator("#livecount")).to_have_attribute("data-unknown", "true")
                        for other in others:
                            item = page.locator(f'#side .item[data-uid="{other}"]')
                            expect(item).to_have_count(1)
                            assert "live" not in (item.get_attribute("class") or "").split(), other
                        page.locator("#livecount").click()
                        expect(page.locator("#console-toast")).to_contain_text("运行状态未知")
                        for other in others:
                            expect(page.locator(f'#side .item[data-uid="{other}"]')).to_have_count(1)
                        assert page.evaluate("S.activeOnly") is False

                    unrelated_stay_unknown()
                    page.locator(f'#side .item[data-uid="{others[1]}"]').click()
                    expect(page.locator("#msgs")).to_contain_text("Synthetic unrelated claude question")
                    expect(page.locator("#dlive")).to_have_attribute("title", "运行状态未知，尚未实现进程探测")
                    expect(page.locator("#a-term")).to_have_attribute("data-unavailable", "true")

                    # Managed instance: the console is available while it runs.
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    expect(page.locator("#msgs")).to_contain_text("Synthetic live managed console")
                    expect(page.locator("#a-term")).to_be_visible()
                    expect(page.locator("#a-term")).to_have_attribute("data-unavailable", "false")
                    page.wait_for_function("instance => (T.list || []).some(row => row.instance_id === instance)", arg=instance)
                    page.locator("#a-term").click()
                    expect(page.locator("#termpane")).to_be_visible()
                    page.wait_for_function("T.ws && T.ws.readyState === WebSocket.OPEN")
                    page.wait_for_function("[...T.views.values()].some(v=>v.term?.buffer?.active && Array.from({length:v.term.buffer.active.length},(_,i)=>v.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes('RS_SHELL_READY'))")

                    # Exit through the console; the host reaps the shell and
                    # removes its record. The UI retires the instance and the
                    # API turns the session exited without touching others.
                    keyboard = page.locator("#termpane .xterm-helper-textarea")
                    keyboard.press_sequentially("quit")
                    keyboard.press("Enter")
                    expect(page.locator("#a-term")).to_have_attribute("data-unavailable", "true", timeout=15000)
                    assert process.wait(timeout=10) == 0
                    assert not (root / "host" / (record["name"] + ".json")).exists()
                    page.wait_for_function("instance => !(T.list || []).some(row => row.instance_id === instance)", arg=instance)
                    assert page.evaluate("uid => T.ended.has(uid)", uid)
                    ended = wait_live(opener, base, lambda d: d["managed"]["sessions"].get(uid, {}).get("state") == "exited")
                    assert ended["uids"] == [] and ended["started_at"] == {} and ended["known"] is True, ended
                    session = ended["managed"]["sessions"][uid]
                    assert session["evidence"] in {"identity_gone", "host_exit"}, session
                    exit_evidence = session["evidence"]
                    assert session["pid"] == record["pid"] and session["instance_id"] == instance, session
                    assert ended["managed"]["hosts"] == []
                    assert all(other not in ended["managed"]["sessions"] for other in others), ended["managed"]["sessions"]
                    unrelated_stay_unknown()
                    expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_have_count(1)

                    # Mobile: the same explicit unknown affordances.
                    page.set_viewport_size({"width": 390, "height": 844})
                    page.goto(base, wait_until="networkidle")
                    expect(page.locator("#session-active")).to_have_text("?")
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    expect(page.locator("#a-term")).to_be_visible()
                    expect(page.locator("#a-term")).to_have_attribute("data-unavailable", "true")
                    assert not errors, errors
                    assert all(url.startswith(base + "/") for url in requests)
                    assert not any("/api/live" in url for url in requests), "legacy UI must not poll /api/live while live:false"
                    context.close()
                assert {name: path.read_bytes() for name, path in corpus.paths.items()} == native
            finally:
                browser.close()
    print(f"PASS live browser: verified running managed instance, exit evidence {exit_evidence}, unrelated sessions unknown (not stopped) on desktop/mobile, no legacy /api/live poll, native fixtures unchanged")


if __name__ == "__main__":
    main()
