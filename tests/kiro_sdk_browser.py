#!/usr/bin/env python3
"""Kiro dual-mode (ACP SDK + CLI PTY) validation through real HTTP/SSE and Chromium.

Exercises:
- Official paginated session/list discovery without reading native files
- Cached read-only selection, explicit load replay and durable history projection
- SDK creation/configuration, streaming, approval/question, cancel and CLI exclusion
- Capability-gated rename, official fork/delete without native-file trash or lock cleanup
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright

from codex_sdk_browser import server
from history_fixtures import BINARY
from kiro_sdk_fixtures import KiroFixture
from sdk_usage_fixtures import assert_inline_usage, assert_usage_outside_menu, refresh_usage
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
    page.wait_for_function("async sid => { await loadSessions(true); return S.sessions.some(row => row.sid === sid); }", arg=fixture.empty_sid, polling=500, timeout=30000)
    rows = page.evaluate("S.sessions")
    kiro_rows = [r for r in rows if r.get("source") == "kiro"]
    assert len(kiro_rows) == 2, kiro_rows
    assert not any(r.get("sid") == fixture.v2_sid for r in rows), "Private legacy files leaked into official catalog"
    v3_row = next(r for r in kiro_rows if r.get("sid") == fixture.sid)
    empty_row = next(r for r in kiro_rows if r.get("sid") == fixture.empty_sid)
    assert v3_row["title"] == "Synthetic Kiro V3 Session", v3_row
    assert v3_row["model"] == "claude-haiku-4.5", v3_row
    assert empty_row["title"] == "新建 Kiro 会话", empty_row
    uid, empty_uid = v3_row["uid"], empty_row["uid"]
    assert any(r.get("params", {}).get("cursor") == "fixture-second-page" for r in fixture.received("session/list"))

    # Selection uses only the app projection. It must not silently activate native SDK.
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    expect(page.locator("#a-chat")).to_be_hidden()
    expect(page.locator(".native-history-warning")).to_contain_text("Kiro 历史尚未同步")
    if screenshots:
        page.screenshot(path=str(screenshots / "kiro-history-not-synced.png"), full_page=True)
        page.set_viewport_size({"width": 390, "height": 900})
        page.evaluate("showMobileDetail()")
        page.emulate_media(color_scheme="dark")
        page.screenshot(path=str(screenshots / "kiro-history-not-synced-mobile.png"), full_page=True)
        page.set_viewport_size({"width": 1280, "height": 900})
        page.emulate_media(color_scheme="light")
    assert not fixture.received("session/load"), "Read-only selection loaded the native session"
    assert not fixture.received("_kiro/session/history"), "Read-only selection initialized native history"

    # Official deletion does not load a session or manufacture a restorable native trash entry.
    loads_before = len(fixture.received("session/load"))
    page.locator(f'#side .item[data-uid="{empty_uid}"]').click()
    expect(page.locator(f'.dhead[data-uid="{empty_uid}"]')).to_be_visible()
    page.wait_for_function("uid => NativeSdkView.uid === uid && NativeSdkView.historyData && !NativeSdkView.historyReady", arg=empty_uid)
    delete_button = page.locator("#a-session-action")
    if not delete_button.is_visible():
        page.locator("#a-more").click()
    # A real background catalog metadata update must not close an open action menu.
    page.evaluate("""uid => {
      S.sessions = S.sessions.map(row => row.uid === uid ? {...row, starred: !row.starred} : row);
      refreshSessionMeta();
    }""", empty_uid)
    expect(page.locator("#session-actions-menu")).to_be_visible()
    expect(page.locator("#a-more")).to_have_attribute("aria-expanded", "true")
    delete_button.click()
    popup = page.locator("dialog.app-popup")
    expect(popup).to_contain_text("Kiro 会话会通过原生接口永久删除，不进回收站，无法恢复。")
    if screenshots:
        for width in (1280, 390):
            page.set_viewport_size({"width": width, "height": 900})
            for theme in ("light", "dark"):
                page.emulate_media(color_scheme=theme)
                page.screenshot(path=str(screenshots / f"kiro-delete-confirm-{width}-{theme}.png"), full_page=True)
        page.set_viewport_size({"width": 1280, "height": 900})
        page.emulate_media(color_scheme="light")
    popup.get_by_role("button", name="取消", exact=True).click()
    assert not fixture.received("session/delete"), "Cancel issued a native delete"
    if not delete_button.is_visible():
        page.locator("#a-more").click()
    delete_button.click()
    with page.expect_response(lambda r: r.request.method == "DELETE" and "/api/session/" in r.url) as deleted:
        page.locator("dialog.app-popup").get_by_role("button", name="确定", exact=True).click()
    assert deleted.value.status == 200, deleted.value.text()
    expect(page.locator("#detail")).to_contain_text("会话已从 Kiro 删除")
    expect(page.locator("#detail-open-trash")).to_have_count(0)
    assert len(fixture.received("session/load")) == loads_before
    assert fixture.received("session/delete")[-1]["params"]["sessionId"] == fixture.empty_sid
    assert not list((fixture.root / "trash").glob("**/session.json")), "Native files were put in app trash"

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

    # 4. Merely choosing SDK stays read-only. A real composer click starts it.
    select_sdk(page, uid)
    assert not fixture.received("session/load"), "Choosing SDK silently loaded the existing session"
    with page.expect_response(lambda r: r.url.endswith("/api/native-sdk/open") and r.request.method == "POST") as opened_response:
        page.locator("#sdk-input").click()
    assert opened_response.value.ok, opened_response.value.text()
    opened = opened_response.value.json()
    assert opened["session_id"] == fixture.sid, opened
    assert len(fixture.received("session/load")) == 1, "Composer click must load the selected native SID once"
    assert fixture.received("session/load")[0]["params"]["sessionId"] == fixture.sid
    replay_items = opened["raw"]["items"]
    assert sum("Explain Kiro V3 ACP architecture" in (item.get("content") or "") for item in replay_items) == 1, replay_items
    assert sum("Kiro V3 uses ACP over JSON-RPC 2.0." in (item.get("content") or "") for item in replay_items) == 1, replay_items
    watch(page, uid)

    # Account credits: a separate zero-turn ACP query fills the footer after open.
    credits = page.locator(".sdk-pane .sdk-status-row > .sdk-gauges .sdk-gauge").filter(has_text="月度")
    expect(credits).to_have_count(1, timeout=30000)
    expect(credits.locator("b")).to_have_text("25%")
    expect(credits.get_by_role("progressbar")).to_have_attribute("aria-label", "KIRO POWER 月度 已用")
    expect(credits).to_have_attribute("title", re.compile(
        r"^KIRO POWER · 月度 已用 25%，剩余 75%；重置剩余 (?:\d+d )?\d{2}:\d{2}；更新于 "))
    expect(page.locator(".sdk-pane .sdk-gauges")).to_have_attribute("title", re.compile(
        r"KIRO POWER · 月度：2,500\.25 / 10,000 Credits · 剩余 75%；重置剩余 (?:\d+d )?\d{2}:\d{2}"))
    assert_inline_usage(page)
    usage_calls = fixture.received("_kiro/account/getUsage")
    assert len(usage_calls) == 1 and usage_calls[0]["params"] == {}, usage_calls
    snapshot = json.loads((fixture.root / "state/quotas/kiro.json").read_text())
    bucket = snapshot["buckets"][0]
    assert bucket["window"] == "monthly" and bucket["used"] == 2500.25 and bucket["limit"] == 10000, snapshot
    assert re.fullmatch(r"\d{4}-\d{2}-01T00:00:00\+00:00", bucket["resets_at"]), bucket
    # Wide footers hold every control, so ⋯ is absent; usage never moves into it.
    expect(page.locator("#sdk-more")).to_be_hidden()
    assert_usage_outside_menu(page)
    if screenshots:
        page.screenshot(path=str(screenshots / "kiro-sdk-credits.png"), full_page=True)
    page.set_viewport_size({"width": 390, "height": 900})
    page.evaluate("showMobileDetail()")
    expect(page.locator("#sdk-input")).to_be_visible()
    expect(page.locator("#sdk-more")).to_be_visible()
    page.locator("#sdk-more").click()
    more = page.locator("#sdk-more-menu")
    expect(more).to_be_visible()
    expect(more.locator(".sdk-gauge, .sdk-inline-usage")).to_have_count(0)
    expect(more).not_to_contain_text("用量")
    expect(more.locator("select:visible").first).to_be_visible()
    if screenshots:
        for theme in ("light", "dark"):
            page.emulate_media(color_scheme=theme)
            page.screenshot(path=str(screenshots / f"kiro-sdk-more-390-{theme}.png"), full_page=True)
        page.emulate_media(color_scheme="light")
    page.keyboard.press("Escape")
    page.set_viewport_size({"width": 1280, "height": 900})
    expect(page.locator("#sdk-more")).to_be_hidden()
    # The web /usage command explicitly refreshes the account snapshot.
    (fixture.root / "kiro-credits-used").write_text("8100")
    refresh_usage(page)
    expect(credits.locator("b")).to_have_text("81%")
    expect(credits).to_have_class(re.compile(r"\blow\b"))
    assert len(fixture.received("_kiro/account/getUsage")) == 2

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

    # No rename extension is advertised; neither title nor private native bytes may be changed.
    native_before = (fixture.v3_dir / "session.json").read_bytes()
    rename_res = api_raw(page, "rename", {"uid": uid, "title": "Unsupported rename"})
    assert rename_res["status"] >= 400 or rename_res["body"].get("ok") is False, rename_res
    assert (fixture.v3_dir / "session.json").read_bytes() == native_before
    assert not fixture.received("_kiro/session/rename")
    # Completed turns and plain usage reads reuse the account snapshot.
    assert len(fixture.received("_kiro/account/getUsage")) == 2, "Turns re-queried account usage"
    assert usage_res["quota"]["buckets"][0]["remaining_percent"] == 19, usage_res["quota"]

    fork_res = api(page, "fork", {"uid": uid, "cwd": str(fixture.work), "request_id": "kiro-fork-1"})
    assert fork_res["uid"] != uid and fork_res["session_id"] != fixture.sid, fork_res
    assert fixture.received("session/fork")[-1]["params"]["sessionId"] == fixture.sid
    forks_after_first = len(fixture.received("session/fork"))
    fork_retry = api(page, "fork", {"uid": uid, "cwd": str(fixture.work), "request_id": "kiro-fork-1"})
    assert fork_retry["uid"] == fork_res["uid"] and fork_retry["session_id"] == fork_res["session_id"], (fork_retry, fork_res)
    assert len(fixture.received("session/fork")) == forks_after_first, "Retry forked the native session twice"
    assert (fixture.v3_dir / "session.json").read_bytes() == native_before

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

    # Private lock-looking files are opaque: no detection, deletion or stale-lock repair.
    api(page, "stop", {"uid": uid})
    lock_path = fixture.sessions / "cli" / f"{fixture.sid}.lock"
    lock_path.write_text("999991")
    reopened = api(page, "open", {"uid": uid})
    assert reopened["session_id"] == fixture.sid
    assert lock_path.read_text() == "999991", "Private native lock was inspected or mutated"
    api(page, "stop", {"uid": uid})

    loads_before_reload = len(fixture.received("session/load"))
    page.reload(wait_until="domcontentloaded")
    page.wait_for_function("T.listLoaded")
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    expect(page.locator("#msgs")).to_contain_text("Explain Kiro V3 ACP architecture")
    expect(page.locator("#msgs")).to_contain_text("Kiro ACP V3 final answer for: Verify Kiro V3 streaming turn")
    assert len(fixture.received("session/load")) == loads_before_reload, "Reloading cached history activated native SDK"

    # Sidebar deletion uses the bulk route even for one row; it must show the same native semantics.
    page.locator(f'#side .item[data-uid="{uid}"]').click(button="right")
    page.locator('#item-menu [data-act="delete"]').click()
    expect(page.locator("dialog.app-popup")).to_contain_text("Kiro 会话会通过原生接口永久删除，不进回收站，无法恢复。")
    deletes_before = len(fixture.received("session/delete"))
    page.locator("dialog.app-popup").get_by_role("button", name="取消", exact=True).click()
    assert len(fixture.received("session/delete")) == deletes_before

    # The stopped session keeps the account snapshot; the Agent quota page lists Kiro too.
    expect(page.locator(".sdk-gauges .sdk-gauge:visible").filter(has_text="月度").locator("b")).to_have_text("81%")
    page.locator("#settings").click()
    page.get_by_role("tab", name="机器", exact=True).click()
    page.locator("#quota-open").click()
    table = page.locator("#quota-dialog").get_by_role("table", name="Kiro", exact=True)
    expect(table.get_by_role("columnheader")).to_have_text(["机器", "窗口", "已使用", "重置倒计时", "采集"])
    expect(table.locator("tbody tr").first).to_contain_text("月度")
    expect(table.locator(".quota-remaining b")).to_have_text("81%")
    expect(table.locator("[data-deadline]")).to_have_text(re.compile(r"^(?:\d+d )?\d{2}:\d{2}$"))
    expect(page.locator("#quota-refresh")).to_be_enabled()
    if screenshots:
        for theme in ("light", "dark"):
            page.emulate_media(color_scheme=theme)
            page.screenshot(path=str(screenshots / f"kiro-quota-table-{theme}.png"), full_page=True)
        page.emulate_media(color_scheme="light")
    page.keyboard.press("Escape")
    expect(page.locator("#quota-dialog")).to_have_count(0)
    page.keyboard.press("Escape")

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

    for row in fixture.received():
        if "fixture_start" in row:
            argv = row["fixture_start"]["argv"]
            assert "--agent-engine=v3" in argv and "--auth-method=cli" in argv, argv
    assert not (fixture.home / ".kiro" / "sessions").exists(), "Service created the opaque native scope path"
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
