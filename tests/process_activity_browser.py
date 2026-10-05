#!/usr/bin/env python3
"""Owned background processes drive breathing dots on a node and through Hub.

Private native records and a synthetic /proc reproduce completed Claude/Codex
turns with detached training, quiet code-mode/MCP/OpenCode services and zombies.
Chromium opens sessions, reloads and observes task completion without changing
the native transcript. No real CLI or production data is touched.
"""

import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import re
import shutil
import tempfile
from types import SimpleNamespace

from playwright.sync_api import sync_playwright, expect
from history_parity import BINARY, Corpus, claude_row, codex_row, codex_message, isolated_server, get_json
from hub_http_suite import Hub, scoped
from node_auth_suite import node_env, TOKEN, free_port
from spawned_by_suite import proc_pid

WORKING = re.compile(r"\bturn-working\b")


def main(binary):
    with tempfile.TemporaryDirectory(prefix="sessiondock-process-activity-") as temporary, ExitStack() as stack:
        root = Path(temporary)
        corpus = Corpus(root / "node")
        for sid in ("busy", "quiet"):
            corpus.put(sid, "codex", [
                codex_row("session_meta", {"id": sid, "cwd": "/synthetic/work"}),
                codex_message("user", "Synthetic process activity " + sid),
                codex_row("event_msg", {"type": "task_complete", "turn_id": sid})], [])
        claude_sid = "0aaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
        corpus.put(claude_sid, "claude", [
            claude_row(claude_sid, "user", "u0", None, "Synthetic idle Claude"),
            claude_row(claude_sid, "assistant", "a0", "u0", "Synthetic completed Claude answer")], [])
        proc = corpus.root / "proc"
        proc.mkdir()
        (proc / "stat").write_text("btime 1700000000\n")
        (corpus.root / "ids").mkdir()
        nid = "a" * 32
        (corpus.root / "ids/node-id").write_text(nid)
        state = corpus.root / "state"
        state.mkdir(mode=0o700)
        proc_pid(proc, 100, "codex", ["codex"], 1, fds={3: str(corpus.paths["busy"])})
        proc_pid(proc, 200, "codex", ["codex"], 1, fds={3: str(corpus.paths["quiet"])})
        # Quiet infrastructure must not turn the dot on. A stale inherited
        # identity on another positively owned CLI must not activate busy.
        proc_pid(proc, 300, "code-host", ["/synthetic/codex-code-mode-host"], 100)
        proc_pid(proc, 400, "mcp", ["mcp-server-filesystem"], 100,
                 env=[("CODEX_SESSION_ID", "busy")])
        proc_pid(proc, 500, "python", ["python", "transport.py"], 400)
        proc_pid(proc, 600, "zombie", [], 100, env=[("CODEX_SESSION_ID", "busy")])
        stat = proc / "600/stat"
        stat.write_text(stat.read_text().replace(") S ", ") Z "))
        proc_pid(proc, 601, "claude", ["claude", "--resume", claude_sid], 1,
                 fds={3: str(corpus.paths[claude_sid])})
        # BUG-20261003-060744-e01ddb: a detached OpenCode service inherited
        # an idle Claude's identity during installation. Neither the service
        # nor its children are evidence that the old session is working.
        proc_pid(proc, 700, "opencode", ["/synthetic/opencode", "serve", "--service"], 1,
                 env=[("CLAUDE_CODE_SESSION_ID", claude_sid)])
        proc_pid(proc, 701, "python", ["python", "transport.py"], 700,
                 env=[("CLAUDE_CODE_SESSION_ID", claude_sid)])
        (proc / "200/environ").write_bytes(b"CODEX_SESSION_ID=busy\0")
        node = SimpleNamespace(name="synthetic", nid=nid, port=free_port(), token=TOKEN)
        env = node_env(corpus.root, node.port, "127.0.0.0/8")
        env["SESSIONDOCK_PROC_ROOT"] = str(proc)
        base, opener = stack.enter_context(isolated_server(corpus, binary, state_dir=state, extra_env=env))
        hubroot = root / "hub"
        hubroot.mkdir()
        hub = Hub(binary.resolve().with_name("sessiondock-hub"), hubroot, [node])
        hub.start()
        stack.callback(hub.stop)
        with sync_playwright() as pw:
            options = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = pw.chromium.launch(**options)
            try:
                for through_hub in (False, True):
                    url = f"http://127.0.0.1:{hub.port}" if through_hub else base
                    uid = scoped(nid, corpus.uid("busy")) if through_hub else corpus.uid("busy")
                    quiet = scoped(nid, corpus.uid("quiet")) if through_hub else corpus.uid("quiet")
                    claude = scoped(nid, corpus.uid(claude_sid)) if through_hub else corpus.uid(claude_sid)
                    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                    context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(url + "/") else route.abort())
                    page = context.new_page()
                    page.goto(url, wait_until="networkidle")
                    badge = page.locator(f'#side .item[data-uid="{uid}"] > .ico > .item-status')
                    quiet_badge = page.locator(f'#side .item[data-uid="{quiet}"] > .ico > .item-status')
                    claude_badge = page.locator(f'#side .item[data-uid="{claude}"] > .ico > .item-status')
                    page.locator(f'#side .item[data-uid="{claude}"]').click()
                    expect(page.locator("#msgs")).to_contain_text("Synthetic completed Claude answer")
                    page.wait_for_function("uid => S.live.has(uid)", arg=claude)
                    expect(page.locator("#dlive")).not_to_have_class(WORKING)
                    expect(claude_badge).not_to_have_class(WORKING)
                    assert get_json(opener, base, "/api/live?force=1")["working_uids"] == []
                    # The same service still stays quiet under a live CLI.
                    stat = proc / "700/stat"
                    stat.write_text(stat.read_text().replace(") S 1 ", ") S 601 "))
                    page.evaluate("refreshLive(true)")
                    expect(claude_badge).not_to_have_class(WORKING)
                    assert get_json(opener, base, "/api/live?force=1")["working_uids"] == []
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    expect(page.locator("#msgs")).to_contain_text("Synthetic process activity busy")
                    page.wait_for_function("uid => S.live.has(uid)", arg=uid)
                    header = page.locator("#dlive")
                    expect(header).not_to_have_class(WORKING)
                    expect(badge).not_to_have_class(WORKING)
                    # OpenCode one-shot commands still count as actual work.
                    proc_pid(proc, 800, "opencode", ["opencode", "run", "Synthetic task"], 601)
                    page.evaluate("refreshLive(true)")
                    expect(claude_badge).to_have_class(WORKING)
                    expect(header).not_to_have_class(WORKING)
                    assert get_json(opener, base, "/api/live?force=1")["working_uids"] == [corpus.uid(claude_sid)]
                    shutil.rmtree(proc / "800")
                    page.evaluate("refreshLive(true)")
                    expect(claude_badge).not_to_have_class(WORKING)
                    # Same-parent commands, including ones under code-mode,
                    # and detached commands with native identity all count.
                    for parent, identity in [(100, []), (300, []), (1, [("CODEX_SESSION_ID", "busy"), ("CODEX_THREAD_ID", "busy")])]:
                        proc_pid(proc, 800, "python", ["python", "train.py"], parent, env=identity)
                        page.evaluate("refreshLive(true)")
                        expect(header).to_have_class(WORKING)
                        expect(badge).to_have_class(WORKING)
                        expect(quiet_badge).not_to_have_class(WORKING)
                        assert get_json(opener, base, "/api/live?force=1")["working_uids"] == [corpus.uid("busy")]
                        assert header.evaluate("e => getComputedStyle(e).animationName") == "turn-pulse"
                        # Task completion changes no native history: only the
                        # new process snapshot must repaint both indicators.
                        shutil.rmtree(proc / "800")
                        page.evaluate("refreshLive(true)")
                        expect(header).not_to_have_class(WORKING)
                        expect(badge).not_to_have_class(WORKING)
                    proc_pid(proc, 800, "sleep", ["sleep", "60"], 1, env=[("CODEX_SESSION_ID", "busy")])
                    page.evaluate("refreshLive(true)")
                    page.reload(wait_until="networkidle")
                    page.wait_for_function("uid => S.sel === uid && S.live.has(uid)", arg=uid)
                    expect(header).to_have_class(WORKING)
                    page.locator(f'#side .item[data-uid="{quiet}"]').click()
                    expect(page.locator("#msgs")).to_contain_text("Synthetic process activity quiet")
                    expect(header).not_to_have_class(WORKING)
                    expect(badge).to_have_class(WORKING)
                    shutil.rmtree(proc / "800")
                    page.evaluate("refreshLive(true)")
                    expect(badge).not_to_have_class(WORKING)
                    context.close()
                    print("PASS", "Hub namespace" if through_hub else "node", "owned/detached/sleeping tasks, infrastructure isolation, reload and completion", flush=True)
            finally:
                browser.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    main(parser.parse_args().binary)
