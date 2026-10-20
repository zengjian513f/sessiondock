#!/usr/bin/env python3
"""Kiro dual-mode (ACP SDK + CLI PTY) validation through real HTTP/SSE and Chromium.

Exercises:
- V3 (`session.json` + `messages.jsonl`) and V2 (`cli/<uuid>.json`) session list & history
- Dual-mode view bar (`#a-chat` hidden, only `#a-sdk` and `#a-console` visible for Kiro)
- Read-only session open without auto-spawning `kiro-cli acp`
- Empty Kiro session `"新建 Kiro 会话"` title and trash/delete
- New Kiro SDK session creation with model & effort configuration
- Streaming thought, tool_call, permission approval (`session/request_permission`),
  user input question (`_kiro/userInput`), settings, compact, usage, rename, fork, cancel
- Mutual exclusion between CLI PTY mode and ACP SDK mode (including stale `.lock` cleanup)
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import os
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright

from codex_sdk_browser import server
from history_fixtures import BINARY
from kiro_sdk_fixtures import KiroFixture
from sdkhost_fixtures import stop_private_sdkhosts
from send_browser import initialize


def api_raw(page, method: str, body: dict):
    return page.evaluate(
        """async ({method, body}) => {
          const r = await fetch('/api/native-sdk/' + method, {
            method: 'POST',
            headers: {'Content-Type': 'application/json'},
            body: JSON.stringify(body),
          });
          return {status: r.status, body: await r.json()};
        }""",
        {"method": method, "body": body},
    )


def api(page, method: str, body: dict):
    res = api_raw(page, method, body)
    assert res["status"] < 300 and res["body"].get("ok") is not False, res
    return res["body"]


def watch(page, uid: str):
    page.evaluate(
        """uid => {
          window.__kiroEvents?.close();
          window.__kiroSnapshot = null;
          const es = window.__kiroEvents = new EventSource('/api/native-sdk/events?uid=' + encodeURIComponent(uid));
          es.addEventListener('sdk', e => {
            const event = JSON.parse(e.data);
            if (event.type === 'snapshot') {
              window.__kiroSnapshot = event.raw;
            } else if (event.type === 'input_queue' && window.__kiroSnapshot) {
              Object.assign(window.__kiroSnapshot, event.raw);
            }
          });
        }""",
        uid,
    )
    page.wait_for_function("window.__kiroSnapshot !== null")


def select_sdk(page, uid: str):
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    # Verify #a-chat is hidden for Kiro sessions!
    expect(page.locator("#a-chat")).to_be_hidden()
    expect(page.locator("#a-sdk")).to_be_visible()
    expect(page.locator("#a-term")).to_be_visible()
    page.locator("#a-sdk").click()
    page.wait_for_function(
        "document.querySelector('#sdk-input')?.getClientRects().length || "
        "(document.querySelector('#a-session-toggle')?.getAttribute('aria-label') === '启动会话' && "
        "!document.querySelector('#a-session-toggle').disabled)"
    )
    if not page.locator("#sdk-input").is_visible() or page.locator("#sdk-input").is_disabled():
        toggle = page.locator('#a-session-toggle[aria-label="启动会话"]')
        if toggle.is_visible():
            toggle.click()
    expect(page.locator("#sdk-input")).to_be_visible()
    expect(page.locator("#sdk-input")).to_be_enabled()


def exercise(browser, base: str, fixture: KiroFixture, screenshots: Path | None):
    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
    context.route(
        "**/*",
        lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort(),
    )
    page = context.new_page()
    page.set_default_timeout(15000)
    failures = []
    page.on("pageerror", lambda error: failures.append(str(error)))

    page.goto(base, wait_until="domcontentloaded")
    page.wait_for_function("T.listLoaded")
    rows = page.evaluate("S.sessions")
    kiro_rows = [r for r in rows if r.get("source") == "kiro"]
    assert len(kiro_rows) == 3, kiro_rows

    v3_row = next(r for r in kiro_rows if r.get("sid") == fixture.sid)
    v2_row = next(r for r in kiro_rows if r.get("sid") == fixture.v2_sid)
    empty_row = next(r for r in kiro_rows if r.get("sid") == fixture.empty_sid)

    assert v3_row["title"] == "Synthetic Kiro V3 Session", v3_row
    assert v3_row["model"] == "claude-haiku-4.5", v3_row
    assert v2_row["title"] == "Synthetic Kiro V2 Legacy", v2_row
    assert v2_row["model"] == "claude-sonnet-4.5", v2_row
    assert empty_row["title"] == "新建 Kiro 会话", empty_row

    uid = v3_row["uid"]
    v2_uid = v2_row["uid"]
    empty_uid = empty_row["uid"]

    # 1. Read-only open: clicking V2 and V3 sessions shows history without spawning kiro-cli acp
    page.locator(f'#side .item[data-uid="{v2_uid}"]').click()
    expect(page.locator("#a-chat")).to_be_hidden()
    expect(page.locator("#msgs")).to_contain_text("Hello from Kiro V2 legacy")
    expect(page.locator("#msgs")).to_contain_text("Kiro V2 legacy response ready.")
    assert not fixture.received("session/load"), "Clicking session auto-spawned kiro-cli acp!"

    page.locator(f'#side .item[data-uid="{uid}"]').click()
    expect(page.locator("#a-chat")).to_be_hidden()
    expect(page.locator("#msgs")).to_contain_text("Explain Kiro V3 ACP architecture")
    expect(page.locator("#msgs")).to_contain_text("Kiro V3 uses ACP over JSON-RPC 2.0.")
    assert not fixture.received("session/load"), "Clicking V3 session auto-spawned kiro-cli acp!"

    # 2. Empty Kiro session trash/delete and restore (e4e3a5ae)
    trash_res = page.evaluate(
        """async uid => {
          const r = await fetch('/api/sessions/delete', {
            method: 'POST',
            headers: {'Content-Type': 'application/json'},
            body: JSON.stringify({uids: [uid], force: true}),
          });
          return {status: r.status, body: await r.json()};
        }""",
        empty_uid,
    )
    assert trash_res["status"] == 200 and len(trash_res["body"].get("deleted", [])) == 1, trash_res
    trash_id = trash_res["body"]["deleted"][0]["entry_id"]
    restore_res = page.evaluate(
        """async id => {
          const r = await fetch('/api/trash/restore', {
            method: 'POST',
            headers: {'Content-Type': 'application/json'},
            body: JSON.stringify({id}),
          });
          return {status: r.status, body: await r.json()};
        }""",
        trash_id,
    )
    assert restore_res["status"] == 200 and restore_res["body"].get("ok") is True, restore_res

    # 3. Create new Kiro SDK session from #new-session modal
    page.locator("#new-session").click()
    page.locator("label:has(input[name=new-source][value=kiro])").click()
    expect(page.locator("#new-model")).to_be_enabled()
    page.locator("#new-model").click()
    page.locator('#new-model-options [title="qwen3-coder-next"]').click()
    page.locator("#new-effort").select_option("low")
    page.locator("#new-cwd").fill(str(fixture.work))
    with page.expect_response(lambda r: r.url.endswith("/api/native-sdk/create")) as created_resp:
        page.locator("#new-session-go").click()
    assert created_resp.value.ok, created_resp.value.text()
    receipt = created_resp.value.json()
    assert receipt["session"]["source"] == "kiro", receipt
    assert receipt["session"]["title"] == "新建 Kiro 会话", receipt
    expect(page.locator("#sdk-input")).to_be_visible()
    expect(page.locator("#a-chat")).to_be_hidden()
    created_open = api(page, "open", {"uid": receipt["uid"]})
    assert created_open["raw"]["model"] == "qwen3-coder-next", created_open["raw"]
    assert created_open["raw"]["effort"] == "low", created_open["raw"]

    if screenshots:
        page.screenshot(path=str(screenshots / "kiro-sdk-new-session.png"), full_page=True)

    # 4. Open existing V3 session in SDK mode & verify replay deduplication
    select_sdk(page, uid)
    opened = api(page, "open", {"uid": uid})
    assert opened["session_id"] == fixture.sid, opened
    assert not any(
        "Replay chunk during session/load" in (item.get("content") or "")
        for item in opened["raw"]["items"]
    ), opened["raw"]["items"]
    watch(page, uid)

    # 5. Standard streaming turn (thought + tool_call + message + metadata)
    page.locator("#sdk-input").fill("Verify Kiro V3 streaming turn")
    page.locator("#sdk-send").click()
    page.wait_for_function(
        "window.__kiroSnapshot && !window.__kiroSnapshot.running && "
        "window.__kiroSnapshot.items.some(i => i.role === 'assistant' && i.content?.includes('Kiro ACP V3 final answer'))"
    )
    expect(page.locator(".sdk-live")).to_contain_text("Kiro ACP V3 final answer for: Verify Kiro V3 streaming turn")
    snap = page.evaluate("window.__kiroSnapshot")
    assert snap["context_usage"]["percent"] == 18.5, snap["context_usage"]

    # 6. Permission approval card (`session/request_permission`)
    page.locator("#sdk-input").fill("permission-test")
    page.locator("#sdk-send").click()
    page.wait_for_function("window.__kiroSnapshot?.requests?.some(request => request.type === 'approval')")
    perm_card = page.locator(".sdk-live [data-sdk-request]").filter(has_text="Execute bash command")
    expect(perm_card).to_be_visible()
    if screenshots:
        page.screenshot(path=str(screenshots / "kiro-sdk-permission.png"), full_page=True)
    perm_card.get_by_role("button", name="Allow once", exact=True).click()
    page.wait_for_function(
        "window.__kiroSnapshot && !window.__kiroSnapshot.running && "
        "window.__kiroSnapshot.items.some(i => i.content?.includes('Permission granted and command executed.'))"
    )
    expect(page.locator(".sdk-live")).to_contain_text("Permission granted and command executed.")
    try:
        expect(perm_card).to_have_count(0)
    except AssertionError:
        print("Kiro approval reconciliation", page.evaluate("""() => ({
          seq: NativeSdkView.seq,
          rawRequests: NativeSdkView.raw?.requests,
          requestMap: [...NativeSdkView.requests],
          watchedRequests: window.__kiroSnapshot?.requests,
          cards: [...document.querySelectorAll('[data-sdk-request]')].map(card => ({
            key: card.dataset.sdkRequest, html: card.outerHTML,
          })),
        })"""), flush=True)
        print("Kiro native prompts", fixture.received("session/prompt"), flush=True)
        print("Kiro browser errors", failures, flush=True)
        raise

    # 7. Question card (`_kiro/userInput`)
    page.locator("#sdk-input").fill("question-test")
    page.locator("#sdk-send").click()
    page.wait_for_function("window.__kiroSnapshot?.requests?.some(request => request.type === 'question')")
    q_card = page.locator(".sdk-live [data-sdk-request]").filter(has_text="Kiro Question")
    expect(q_card).to_be_visible()
    q_card.get_by_label("ACP V3", exact=False).first.check()
    if screenshots:
        page.screenshot(path=str(screenshots / "kiro-sdk-question.png"), full_page=True)
    q_card.get_by_role("button", name="提交", exact=True).click()
    page.wait_for_function(
        "window.__kiroSnapshot && !window.__kiroSnapshot.running && "
        "window.__kiroSnapshot.items.some(i => i.content?.includes('User selected Kiro option'))"
    )
    expect(page.locator(".sdk-live")).to_contain_text("User selected Kiro option: ACP V3")

    # 8. Immediate settings (model, effort, mode, autopilot) without triggering a turn
    prompts_before = len(fixture.received("session/prompt"))
    settings_res = api(page, "settings", {
        "uid": uid,
        "model": "qwen3-coder-next",
        "effort": "medium",
        "mode": "plan",
        "autopilot": True,
    })
    assert settings_res["raw"]["model"] == "qwen3-coder-next", settings_res["raw"]
    assert settings_res["raw"]["effort"] == "medium", settings_res["raw"]
    assert settings_res["raw"]["mode"] == "plan", settings_res["raw"]
    assert settings_res["raw"]["autopilot"] is True, settings_res["raw"]
    assert len(fixture.received("session/prompt")) == prompts_before

    # 9. Compact, Usage, Rename, Fork
    compact_res = api(page, "compact", {"uid": uid})
    assert compact_res["raw"]["compact"]["state"] == "success", compact_res["raw"]
    assert fixture.received("_kiro/session/compact")

    usage_res = api(page, "usage", {"uid": uid})
    assert usage_res["context_usage"]["percent"] is not None, usage_res

    rename_res = api(page, "rename", {"uid": uid, "title": "Renamed Kiro V3 Session"})
    assert rename_res["title"] == "Renamed Kiro V3 Session"
    disk_meta = json.loads((fixture.v3_dir / "session.json").read_text())
    assert disk_meta["title"] == "Renamed Kiro V3 Session"

    fork_res = api(page, "fork", {"uid": uid, "cwd": str(fixture.work), "request_id": "kiro-fork-1"})
    assert fork_res["uid"] != uid and fork_res["session_id"] != fixture.sid, fork_res
    assert (fixture.sessions / "bucket_v3" / fork_res["session_id"] / "session.json").is_file()

    # 10. Interrupt / Cancel (`session/cancel`) preserves interrupted state
    page.locator("#sdk-input").fill("wait-for-cancel")
    page.locator("#sdk-send").click()
    page.wait_for_function("window.__kiroSnapshot?.running === true")
    page.locator("#sdk-stop").click()
    page.wait_for_function("window.__kiroSnapshot && !window.__kiroSnapshot.running")
    snap_after_cancel = page.evaluate("window.__kiroSnapshot")
    assert snap_after_cancel["last_turn"]["status"] == "interrupted", snap_after_cancel["last_turn"]
    assert fixture.received("session/cancel")

    # 11. Mutual Exclusion: SDK active -> CLI takeover returns 409 sdk_session_busy
    cli_spawn_while_sdk = page.evaluate(
        """async ({uid}) => {
          const r = await fetch('/api/term/takeover', {
            method: 'POST',
            headers: {'Content-Type': 'application/json'},
            body: JSON.stringify({uid}),
          });
          return {status: r.status, body: await r.json()};
        }""",
        {"uid": uid},
    )
    assert cli_spawn_while_sdk["status"] == 409, cli_spawn_while_sdk
    assert cli_spawn_while_sdk["body"].get("code") == "sdk_session_busy", cli_spawn_while_sdk

    # 12. Stop SDK session -> test stale .lock cleanup vs live .lock conflict
    api(page, "stop", {"uid": uid})
    lock_path = fixture.sessions / "cli" / f"{fixture.sid}.lock"

    # Stale lock (dead PID 999991 not in /proc) -> automatically cleaned up on open
    lock_path.write_text("999991")
    reopened = api(page, "open", {"uid": uid})
    assert reopened["session_id"] == fixture.sid
    assert not lock_path.exists(), "Stale .lock file was not cleaned up!"
    api(page, "stop", {"uid": uid})

    # Live lock (PID 888881 present in fixture.proc) -> refused with kiro_sdk_session_busy
    live_pid = 888881
    proc_dir = fixture.proc / str(live_pid)
    proc_dir.mkdir(exist_ok=True)
    (proc_dir / "stat").write_text(f"{live_pid} (kiro-cli) S 1 " + "0 " * 30 + "\n")
    lock_path.write_text(str(live_pid))
    busy_open = api_raw(page, "open", {"uid": uid})
    assert busy_open["status"] >= 400 or busy_open["body"].get("ok") is False, busy_open
    assert "kiro_sdk_session_busy" in json.dumps(busy_open["body"]), busy_open
    lock_path.unlink(missing_ok=True)

    if screenshots:
        for width in (1280, 390):
            page.set_viewport_size({"width": width, "height": 900})
            if width == 390:
                page.evaluate("showMobileDetail()")
            for theme in ("light", "dark"):
                page.emulate_media(color_scheme=theme)
                page.screenshot(
                    path=str(screenshots / f"kiro-sdk-{width}-{theme}.png"),
                    full_page=True,
                )

    assert not failures, failures
    context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="sessiondock-kiro-sdk-") as temporary:
        root = Path(temporary)
        for name in ("claude", "codex", "grok"):
            (root / name).mkdir()
        ptyhost_bin = args.binary.resolve(strict=True).with_name("ptyhost")
        fixture = KiroFixture(root, host_binary=ptyhost_bin)
        initialize("--initialize-lifecycle", root / "ledger", args.binary.resolve(strict=True))
        (root / "state").mkdir()
        (root / "trash").mkdir()
        extra = {
            **fixture.env,
            "SESSIONDOCK_NATIVE_SDKS": "auto",
            "SESSIONDOCK_LIFECYCLE_DIR": str(root / "ledger"),
            "SESSIONDOCK_PTYHOST_DIR": str(fixture.host_dir),
            "SESSIONDOCK_STATE_DIR": str(root / "state"),
            "SESSIONDOCK_TRASH_DIR": str(root / "trash"),
        }
        try:
            with server(SimpleNamespace(root=root), args.binary, "off", extra) as base:
                with sync_playwright() as playwright:
                    browser = playwright.chromium.launch(headless=True)
                    exercise(browser, base, fixture, args.screenshots)
                    browser.close()
        finally:
            stop_private_sdkhosts(root)
    print("Kiro native ACP SDK + dual-mode browser: PASS")


if __name__ == "__main__":
    main()
