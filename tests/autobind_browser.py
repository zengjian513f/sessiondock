#!/usr/bin/env python3
"""Process-evidence autobind must honour Python's `before` set.

A pending Codex pane whose process holds an *older* rollout open (Codex reads
old rollouts for its resume picker) must stay pending; once the pane writes
and holds its own fresh rollout, the page follows that binding by itself.
Synthetic shell adapter and fixtures only, never a model CLI."""

from contextlib import ExitStack
import json
import os
import shutil
from pathlib import Path
import tempfile
from urllib.parse import urlsplit
import subprocess
import time
from playwright.sync_api import sync_playwright, expect
from history_parity import REPO, BINARY, Corpus, codex_row, codex_message, isolated_server
from terminal_exit_browser import XTERM_TEXT
from hub_send_browser import cleanup_hosts
from popups import on_popup  # noqa: E402

PANE_SCRIPT = """exec 3< "$SESSIONDOCK_TEST_OLD"
stty -echo
printf 'RS_SHELL_READY\\n'
while IFS= read -r command; do
  case "$command" in
    first)
      now=$(date -u +%Y-%m-%dT%H:%M:%S.000Z)
      mkdir -p "$(dirname "$SESSIONDOCK_TEST_NEW")"
      printf '{"type":"session_meta","payload":{"id":"fresh","cwd":"%s","timestamp":"%s"},"timestamp":"%s"}\\n' "$PWD" "$now" "$now" > "$SESSIONDOCK_TEST_NEW"
      printf '{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"first prompt"}]},"timestamp":"%s"}\\n' "$now" >> "$SESSIONDOCK_TEST_NEW"
      exec 4< "$SESSIONDOCK_TEST_NEW"
      printf 'RS_FIRST_OK\\n' ;;
    *) printf 'RS_UNKNOWN_INPUT\\n' ;;
  esac
done
"""


def binding(root, record_id):
    return json.loads((root / "ledger/lifecycle-ledger.json").read_text())["records"][record_id]["binding"]


def main():
    if os.name != "posix":
        raise SystemExit("autobind acceptance requires /proc; no Windows/macOS claim.")
    with tempfile.TemporaryDirectory(prefix="sessiondock-autobind-ui-") as temporary, ExitStack() as cleanup:
        root = Path(temporary)
        # A detached host outlives the web server; retain its guarded socket
        # until cleanup finishes, on both success and failure.
        cleanup.callback(cleanup_hosts, root)
        for name in ["host", "work", "ledger", "claude", "codex", "grok", "audit"]:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        corpus.put("older", "codex", [codex_row("session_meta", {"id": "older", "cwd": str(root / "work"),
                                                          "timestamp": "2026-09-11T10:00:00Z"}),
                                      codex_message("user", "Pre-existing session")], [])
        older_uid = corpus.uid("older")
        fresh_path = root / "codex/2026/09/29/rollout-fresh.jsonl"
        configuration = root / "launcher.json"
        configuration.touch(mode=0o600)
        # The process scan reads only CLI-named processes: a `codex`-named sh.
        (root / "bin").mkdir(mode=0o700)
        fake_codex = root / "bin/codex"
        shutil.copy2(Path("/bin/sh").resolve(), fake_codex)
        profile = {"id": "codex-cli-v1", "source": "codex", "executable": str(fake_codex),
                   "args": ["-c", PANE_SCRIPT], "new_args": [], "resume_args": ["resume", "{sid}"],
                   "env": {"PATH": "/usr/bin:/bin", "TERM": "xterm-256color",
                           "SESSIONDOCK_TEST_OLD": str(corpus.paths["older"]),
                           "SESSIONDOCK_TEST_NEW": str(fresh_path)}}
        configuration.write_text(json.dumps({"schema": 2, "host_binary": str(REPO / "target/debug/ptyhost"),
                                             "host_dir": str(root / "host"), "adapters": [],
                                             "profiles": [profile]}))
        initialized = subprocess.run([str(BINARY), "--initialize-lifecycle", str(root / "ledger")],
                                     cwd=REPO, env={"PATH": "/usr/bin:/bin"}, capture_output=True, timeout=15)
        assert initialized.returncode == 0, initialized.stderr.decode()
        with sync_playwright() as playwright:
            options = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**options)
            try:
                with isolated_server(corpus, BINARY, host_dir=root / "host", lifecycle_dir=root / "ledger",
                                     launcher_config=configuration, audit_dir=root / "audit",
                                     extra_env={"SESSIONDOCK_PROC_SCAN": "1"}) as (base, _):
                    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                    context.route("**/*", lambda route: route.continue_()
                                  if route.request.url.startswith(base + "/") else route.abort())
                    page = context.new_page()
                    errors = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    on_popup(page, lambda dialog: dialog.accept())
                    page.goto(base, wait_until="networkidle")
                    page.locator("#new-session").click()
                    page.locator('input[name="new-source"][value="codex"]').check()
                    page.locator("#new-cwd").fill(str(root / "work"))
                    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/term/create") as created:
                        page.locator("#new-session-go").click()
                    assert created.value.status == 200, created.value.text()
                    receipt = created.value.json()
                    assert receipt["launch_kind"] == "new_pending", receipt
                    pending_uid = "tmux:" + receipt["name"]
                    page.wait_for_function("uid => S.sel === uid", arg=pending_uid)
                    expect(page.locator("#termpane")).to_be_hidden()
                    page.locator("#a-term").click()
                    expect(page.locator("#termpane")).to_be_visible()
                    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
                    page.wait_for_function("(" + XTERM_TEXT + ")().includes('RS_SHELL_READY')")
                    # The pane now holds the older rollout open. Several autobind
                    # passes (6 s idle, then 1.5 s) must leave the receipt pending.
                    deadline = time.monotonic() + 15
                    while time.monotonic() < deadline:
                        assert binding(root, receipt["record_id"]) is None, binding(root, receipt["record_id"])
                        time.sleep(0.5)
                    assert page.evaluate("S.sel") == pending_uid
                    expect(page.locator(f'#side .item[data-uid="{pending_uid}"]')).to_have_count(1)
                    # The user's first prompt writes the pane's own rollout; the
                    # page follows that binding without any operator action.
                    page.locator("#termpane .xterm-helper-textarea").press_sequentially("first")
                    page.locator("#termpane .xterm-helper-textarea").press("Enter")
                    page.wait_for_function("(" + XTERM_TEXT + ")().includes('RS_FIRST_OK')")
                    corpus.paths["fresh"] = fresh_path
                    fresh_uid = corpus.uid("fresh")
                    try:
                        page.wait_for_function("uid => S.sel && S.sel !== uid && !S.sel.startsWith('tmux:')",
                                               arg=pending_uid, timeout=30000)
                    except Exception:
                        for path in sorted((root / "audit").rglob("*")):
                            print("AUDIT", path)
                            if path.is_file():
                                for line in path.read_text(errors="replace").splitlines():
                                    if "lifecycle" in line or "bind" in line:
                                        print(line[:600])
                        print(json.dumps(json.loads((root / "ledger/lifecycle-ledger.json").read_text())["records"])[:1500])
                        print(json.dumps(context.request.get(base + "/api/live").json())[:1500])
                        raise
                    selected = page.evaluate("S.sel")
                    assert selected != older_uid, selected
                    bound = binding(root, receipt["record_id"])
                    assert bound and bound["state"] == "confirmed", bound
                    assert bound["spec"]["sid"] == "fresh", bound
                    assert selected == fresh_uid, (selected, fresh_uid)
                    expect(page.locator(f'#side .item[data-uid="{pending_uid}"]')).to_have_count(0)
                    assert not errors, errors
                    context.close()
            finally:
                browser.close()
    print("autobind_browser: PASS")


if __name__ == "__main__":
    main()
