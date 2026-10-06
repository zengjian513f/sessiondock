#!/usr/bin/env python3
"""Browser-launched test CLIs are reaped on success, failure and SIGTERM.

The fixture owns hosts across server restarts, not the server process itself.
All instances use private temporary directories and free fake CLIs.
"""
import json
import os
from pathlib import Path
import signal
import sys
import tempfile

from playwright.sync_api import sync_playwright
from history_fixtures import BINARY, Corpus, isolated_server
from hub_send_browser import prepare
from private_hosts import private_hosts, running_identity
from run_validation import run_one
from send_browser import create_claude, wait_history


def main(timeout_root=None):
    with sync_playwright() as pw:
        options = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = pw.chromium.launch(**options)
        try:
            for ending in (("timeout",) if timeout_root else ("success", "assertion", "sigterm")):
                with tempfile.TemporaryDirectory(prefix="sessiondock-host-cleanup-") as temporary:
                    root = Path(temporary)
                    launcher = prepare(root)
                    processes = {}
                    previous = signal.getsignal(signal.SIGTERM)
                    try:
                        with private_hosts(root):
                            def server():
                                return isolated_server(Corpus(root), BINARY, host_dir=root / "host",
                                    lifecycle_dir=root / "ledger", launcher_config=launcher,
                                    state_dir=root / "state")

                            with server() as (base, _):
                                page = browser.new_page()
                                page.goto(base, wait_until="networkidle")
                                receipt = create_claude(page, base, root / "work")
                                page.locator("#a-term").click()
                                page.locator("#cinput").fill("fixture cleanup probe")
                                page.locator("#csend").click()
                                wait_history(page, "fixture cleanup probe")
                                uid = page.evaluate("S.sel")
                                page.close()
                                record = json.loads((root / "host" / (receipt["name"] + ".json")).read_text())
                                for pid in (record["host_pid"], record["pid"]):
                                    processes[pid] = running_identity(pid)
                                    assert processes[pid] is not None
                            # A service restart must preserve the host until the
                            # outer fixture finishes, including its final failure.
                            assert all(running_identity(pid) == identity for pid, identity in processes.items())
                            with server() as (base, _):
                                page = browser.new_page()
                                page.goto(base, wait_until="networkidle")
                                page.locator(f'#side .item[data-uid="{uid}"]').click()
                                page.locator("#a-term").click()
                                page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
                                page.close()
                            if ending == "assertion":
                                raise AssertionError("expected fixture failure")
                            if ending == "sigterm":
                                os.kill(os.getpid(), signal.SIGTERM)
                            if ending == "timeout":
                                (timeout_root / "pids.json").write_text(json.dumps(processes))
                                signal.pause()  # The validation runner ends this fixture.
                    except AssertionError as error:
                        assert ending == "assertion" and str(error) == "expected fixture failure", error
                    except SystemExit as error:
                        assert ending in ("sigterm", "timeout") and error.code == 128 + signal.SIGTERM, error
                    assert signal.getsignal(signal.SIGTERM) == previous
                    assert not list((root / "host").glob("*.sock"))
                    assert all(running_identity(pid) != identity for pid, identity in processes.items())
                    print("PASS private host cleanup:", ending, "and service restart", flush=True)
        finally:
            browser.close()
    if timeout_root is None:
        with tempfile.TemporaryDirectory(prefix="sessiondock-host-timeout-") as temporary:
            root = Path(temporary)
            suite = {"name": "cleanup-timeout", "skip": None, "timeout": 30,
                     "argv": [sys.executable, str(Path(__file__).resolve()), "--timeout-worker", str(root)]}
            status, _, _ = run_one(suite, os.environ.copy(), root, 1)
            assert status == "TIMEOUT", status
            processes = json.loads((root / "pids.json").read_text())
            assert all(running_identity(int(pid)) != identity for pid, identity in processes.items())
            assert "PASS private host cleanup: timeout" in (root / "cleanup-timeout.log").read_text()
            print("PASS validation timeout reaps browser-created private hosts", flush=True)


if __name__ == "__main__":
    main(Path(sys.argv[2]) if len(sys.argv) == 3 and sys.argv[1] == "--timeout-worker" else None)
