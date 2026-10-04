#!/usr/bin/env python3
"""Agy models, versions and updates through Chromium on a private node and Hub.

Only synthetic CLIs and curl are executed. Two real loopback nodes exercise
installed/missing CLI gating; the real Hub proxies to them. No business
function is invoked from JavaScript, and no real CLI home is used. This suite
does not establish acceptance of the rest of the Agy integration.
"""
from __future__ import annotations

import argparse
from contextlib import ExitStack, contextmanager
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import traceback
from types import SimpleNamespace
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

from playwright.sync_api import expect, sync_playwright

from browser_runtime import js
from client_update_browser import matrix_row, open_machines
from history_parity import BINARY, REPO, Corpus, isolated_server
from hub_http_suite import Hub, free_port
from new_session_model_browser import choose_model, open_dialog

OLD, NEW = "1.2.16", "1.2.17"
EFFORTS = ["low", "medium", "high", "xhigh", "max"]
PROFILE_ARGS = ["--fixture-profile", "agy"]
MANIFEST_URL = (
    "https://antigravity-cli-auto-updater-974169037036.us-central1.run.app"
    "/manifests/linux_amd64.json"
)
CODEX_URL = "https://registry.npmjs.org/@openai/codex/latest"
MODELS = [("agy-fixture-a", "Agy Fixture A"), ("agy-fixture-b", "Agy Fixture B"),
          ("agy-fixture-unnamed", "agy-fixture-unnamed")]
CHECKS = []
PHASE = "setup"

# The same executable implements installed Agy, missing Agy (127), and an
# existing Codex profile. It cannot invoke a real CLI or open a network socket.
FAKE_CLI = r'''
import json, os, signal, sys, time
from pathlib import Path
state = Path(os.environ['AGY_CLIENTS_STATE'])
source = Path(sys.argv[0]).name
args = sys.argv[1:]
entry = {'source': source, 'argv': args, 'pid': os.getpid(),
         'home': os.environ['HOME'], 'cwd': os.getcwd(),
         'marker': os.environ.get('AGY_CLIENTS_PROFILE')}
def record():
    with (state / 'cli.jsonl').open('a') as log:
        log.write(json.dumps(entry) + '\n')
if source == 'missing-agy':
    record()
    sys.exit(127)
if args[-1:] == ['--version']:
    record()
    print((state / 'agy.version').read_text() if source == 'agy' else 'codex-cli 0.159.0', flush=True)
    sys.exit(0)
if source == 'agy' and args[-1:] == ['models']:
    record()
    print('agy-fixture-a\tAgy Fixture A\nagy-fixture-b\tAgy Fixture B\n'
          'agy-fixture-unnamed\t\nignored diagnostic\n\tmissing id', flush=True)
    sys.exit(0)
if source == 'agy' and args[-1:] == ['update']:
    # Closed stdin must reach EOF immediately; bound a regression here too.
    signal.alarm(3)
    entry['stdin_closed'] = os.read(0, 1) == b''
    signal.alarm(0)
    record()
    print('\033[32mUpdating Agy\033[0m\n10%\r100%', flush=True)
    time.sleep(1.2)
    if (state / 'update.fail').exists():
        print('Error: synthetic Agy updater failure', file=sys.stderr, flush=True)
        sys.exit(7)
    (state / 'agy.version').write_text(os.environ['AGY_CLIENTS_NEW'])
    print('Agy installed ' + os.environ['AGY_CLIENTS_NEW'], flush=True)
    sys.exit(0)
entry['interactive'] = sys.stdin.isatty()
record()
if source != 'agy' or not entry['interactive']:
    sys.exit(2)
print('AGY_CLIENTS_READY', flush=True)
while os.read(0, 4096):
    pass
'''

FAKE_CURL = r'''
import json, os, sys
from pathlib import Path
url = sys.argv[-1]
with (Path(os.environ['AGY_CLIENTS_STATE']) / 'curl.jsonl').open('a') as log:
    log.write(json.dumps({'argv': sys.argv[1:], 'url': url,
        'home': os.environ['HOME'], 'marker': os.environ.get('AGY_CLIENTS_PROFILE')}) + '\n')
if url == os.environ['AGY_CLIENTS_MANIFEST']:
    print(json.dumps({'version': os.environ['AGY_CLIENTS_NEW']}))
elif url == 'https://registry.npmjs.org/@openai/codex/latest':
    print('{"version":"0.159.0"}')
else:
    print('Fixture refused unexpected URL', file=sys.stderr)
    sys.exit(22)
'''


def phase(name):
    global PHASE
    PHASE = name
    print("RUN " + name, flush=True)


def passed(name):
    CHECKS.append(name)
    print("PASS " + name, flush=True)


def records(path):
    return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []


def private_env(root):
    return {"HOME": str(root / "home"), "PATH": str(root / "bin") + ":/usr/bin:/bin",
            "SHELL": "/bin/sh", "TERM": "xterm-256color", "LANG": "C.UTF-8",
            "XDG_CONFIG_HOME": str(root / "home/config"),
            "XDG_DATA_HOME": str(root / "home/data"), "XDG_CACHE_HOME": str(root / "home/cache"),
            "AGY_CLIENTS_STATE": str(root / "fixture-state"), "AGY_CLIENTS_NEW": NEW,
            "AGY_CLIENTS_PROFILE": "private-agy-profile", "AGY_CLIENTS_MANIFEST": MANIFEST_URL}


@contextmanager
def node_fixture(root, args, name, *, installed):
    root.mkdir(mode=0o700)
    for directory in ("home", "bin", "fixture-state", "host", "ledger", "state", "work",
                      "claude", "codex", "grok", "opencode", "agy", "native-agy"):
        (root / directory).mkdir(mode=0o700)
    env = private_env(root)
    for executable, body in (("agy", FAKE_CLI), ("missing-agy", FAKE_CLI),
                             ("fake-codex", FAKE_CLI), ("curl", FAKE_CURL)):
        path = root / "bin" / executable
        path.write_text("#!" + str(Path(sys.executable).resolve()) + "\n" + body)
        path.chmod(0o700)
    (root / "fixture-state/agy.version").write_text(OLD)
    config = root / "launcher.json"
    config.touch(mode=0o600)
    config.write_text(json.dumps({"schema": 2, "host_binary": str(args.ptyhost),
        "host_dir": str(root / "host"), "adapters": [], "profiles": [
            {"id": "agy-cli-v1", "source": "agy", "args": PROFILE_ARGS, "env": env,
             "executable": str(root / "bin" / ("agy" if installed else "missing-agy"))},
            {"id": "codex-cli-v1", "source": "codex", "args": [], "env": env,
             "executable": str(root / "bin/fake-codex")}]}, ensure_ascii=False))
    initialized = subprocess.run([str(args.binary), "--initialize-lifecycle", str(root / "ledger")],
                                 env=env, cwd=root / "work", capture_output=True, timeout=20)
    assert initialized.returncode == 0, initialized.stderr.decode("utf-8", "replace")[:1200]
    token = "t" * 40
    token_file = root / "node.token"
    token_file.touch(mode=0o600)
    token_file.write_text(token + "\n")
    port = free_port()
    try:
        with ExitStack() as server:
            # isolated_server inherits this clean environment before it starts;
            # the patch need not persist while the other node or Hub is created.
            with patch.dict(os.environ, env, clear=True):
                base, _ = server.enter_context(isolated_server(Corpus(root), args.binary,
                    host_dir=root / "host", lifecycle_dir=root / "ledger", launcher_config=config,
                    state_dir=root / "state", file_roots=(root / "work",),
                    file_write_roots=(root / "work",), trash_dir=root / "trash", audit_dir=root / "audit",
                    extra_env={"SESSIONDOCK_NODE_BIND": f"127.0.0.1:{port}",
                        "SESSIONDOCK_NODE_TOKEN_FILE": str(token_file),
                        "SESSIONDOCK_NODE_ID_FILE": str(root / "node.id"),
                        "SESSIONDOCK_NODE_PEERS": "127.0.0.0/8",
                        "SESSIONDOCK_OPENCODE_ROOT": str(root / "opencode"),
                        "SESSIONDOCK_OPENCODE_DB": str(root / "opencode/empty.db"),
                        "SESSIONDOCK_AGY_HOME": str(root / "native-agy"),
                        "SESSIONDOCK_AGY_ROOT": str(root / "agy")}))
            yield SimpleNamespace(name=name, port=port, token=token, base=base, root=root,
                                  state=root / "fixture-state")
    finally:
        for record in (root / "host").glob("sessiondock-*.json"):
            subprocess.run([str(args.ptyhost), "--dir", str(root / "host"), "kill", record.stem, "--force"],
                           env=env, cwd=root / "work", timeout=5, check=False,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


@contextmanager
def page_fixture(browser, base):
    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
    errors = []
    context.route("**/*", lambda route: route.continue_()
                  if route.request.url.startswith(base + "/") else route.abort())
    page = context.new_page()
    page.set_default_timeout(15000)
    page.on("pageerror", lambda error: errors.append(str(error)))
    try:
        page.goto(base + "/", wait_until="networkidle")
        page.wait_for_function(js("T.listLoaded", "runtime.terminal.state.listLoaded"))
        yield page
        assert not errors, errors
    finally:
        context.close()


def check_picker(page, node, *, node_id=None, levels=("",)):
    phase(("Hub" if node_id else "local") + " Agy models and launch choices")
    catalogs = []
    def catalog(response):
        url = urlsplit(response.url)
        if url.path.endswith("/api/term/models") and parse_qs(url.query).get("source") == ["agy"]:
            catalogs.append(response)
    page.on("response", catalog)
    try:
        for effort in levels:
            open_dialog(page)
            if node_id:
                page.locator("#new-node").select_option(node_id)
            radio = page.locator('input[name="new-source"][value="agy"]')
            expect(radio).to_be_enabled()
            label = page.locator('#new-session-form label:has(input[value="agy"])')
            expect(label).to_contain_text("Agy")
            label.click()
            expect(radio).to_be_checked()
            expect(page.locator("#new-model")).to_be_enabled()
            expect(page.locator("#new-effort option")).to_have_text(["CLI 默认", *EFFORTS])
            assert page.locator("#new-effort option").evaluate_all("items => items.map(o => o.value)") == ["", *EFFORTS]
            if effort == "":
                expect(page.locator("#new-effort")).to_have_value("")
            page.locator("#new-model").click()
            expect(page.locator("#new-model-options [role=option] > span")).to_have_text([name for _, name in MODELS])
            page.keyboard.press("Escape")
            choose_model(page, MODELS[0][1])
            page.locator("#new-effort").select_option(effort)
            expect(page.locator("#new-effort")).to_have_value(effort)
            page.locator("#new-cwd").fill(str(node.root / "work"))
            previous = {entry["pid"] for entry in records(node.state / "cli.jsonl") if entry.get("interactive")}
            with page.expect_response(lambda r: urlsplit(r.url).path.endswith("/api/term/create")) as created:
                page.locator("#new-session-go").click()
            response = created.value
            assert response.status == 200, response.text()[:1200]
            request, receipt = response.request.post_data_json, response.json()
            assert request["source"] == "agy" and request["model"] == MODELS[0][0], request
            assert request.get("effort", "") == effort, request
            if not effort:
                assert not request.get("effort"), request
            assert receipt["running"] and receipt["launch_kind"] == "new_pending", receipt
            assert not receipt.get("declared_sid"), receipt
            expect(page.locator("#new-session-dialog")).to_be_hidden()
            deadline = time.monotonic() + 10
            while True:
                launches = [entry for entry in records(node.state / "cli.jsonl")
                            if entry.get("interactive") and entry["pid"] not in previous]
                if launches:
                    break
                assert time.monotonic() < deadline, "created fake CLI did not record its interactive argv"
                time.sleep(0.05)
            expected = PROFILE_ARGS + ["--model", MODELS[0][0]] + (["--effort", effort] if effort else [])
            assert len(launches) == 1 and launches[0]["argv"] == expected, (expected, launches)
            assert launches[-1]["cwd"] == str(node.root / "work"), launches[-1]
            print("  create effort=" + (effort or "CLI default"), flush=True)
        assert catalogs, "UI never requested the Agy catalog"
        body = catalogs[0].json()
        assert catalogs[0].status == 200 and body.get("default_model") is None, body
        assert body["efforts"] == EFFORTS, body
        assert [(m["id"], m["name"]) for m in body["models"]] == MODELS, body
        assert all(m["efforts"] == EFFORTS and "default_effort" not in m for m in body["models"]), body
        if node_id:
            assert parse_qs(urlsplit(catalogs[0].url).query).get("node") == [node_id], catalogs[0].url
        passed(("Hub" if node_id else "local") + ": TSV catalog, no default effort, real create and profile argv "
               + repr(list(levels)))
    finally:
        page.remove_listener("response", catalog)


def check_updates(page, node, machine, *, node_id=None):
    phase(machine + " versions and update success/failure")
    (node.state / "agy.version").write_text(OLD)
    open_machines(page)
    expect(page.locator("#client-matrix thead th")).to_have_text(["机器", "Codex", "Agy"])
    cell = matrix_row(page, machine).locator('td[data-client-source="agy"]')
    expect(cell.locator(".client-version")).to_have_text(OLD)
    expect(cell).to_have_attribute("data-state", "outdated")
    assert "可更新到 " + NEW in (cell.get_attribute("title") or "")
    codex = matrix_row(page, machine).locator('td[data-client-source="codex"]')
    expect(codex.locator(".client-version")).to_have_text("0.159.0")
    expect(codex).to_have_attribute("data-state", "current")
    curl = records(node.state / "curl.jsonl")
    official = [entry for entry in curl if entry["url"] == MANIFEST_URL]
    assert official, "official linux_amd64 Agy manifest fixture was not called"
    assert all(entry["url"] in (MANIFEST_URL, CODEX_URL) for entry in curl), curl
    assert all(entry["home"] == str(node.root / "home") and entry["marker"] == "private-agy-profile"
               for entry in official), official
    assert any(entry["source"] == "agy" and entry["argv"] == PROFILE_ARGS + ["--version"]
               for entry in records(node.state / "cli.jsonl")), "Agy version was not probed"
    for failing in (False, True):
        flag = node.state / "update.fail"
        if failing:
            flag.touch(mode=0o600)
        try:
            with page.expect_response(lambda r: urlsplit(r.url).path.endswith("/api/clients/update")) as updated:
                cell.locator(".client-update").click()
            response = updated.value
            assert response.status == 200 and response.json()["started"], response.text()[:1200]
            assert response.request.post_data_json == {"id": "agy-cli-v1"}, response.request.post_data_json
            expected_path = (f"/api/nodes/{node_id}" if node_id else "") + "/api/clients/update"
            assert urlsplit(response.url).path == expected_path, response.url
            expect(cell.locator(".client-update")).to_have_text("…")
            expect(cell.locator(".client-update")).to_be_disabled()
            note = page.locator("#machine-note")
            if failing:
                expect(note).to_have_text(machine + "：Agy 更新失败（退出码 7）：Error: synthetic Agy updater failure",
                                          timeout=20000)
                expect(note).to_have_attribute("data-state", "error")
                assert "Error: synthetic Agy updater failure" in (cell.get_attribute("title") or "")
            else:
                expect(note).to_have_text(f"{machine}：Agy 已更新 {OLD} → {NEW}。", timeout=20000)
                title = cell.get_attribute("title") or ""
                assert "Agy installed " + NEW in title and "\x1b" not in title, title
                assert "已是最新（" + NEW + "）" in title, title
            expect(cell.locator(".client-version")).to_have_text(NEW)
            expect(cell.locator(".client-update")).to_be_enabled()
            update = [entry for entry in records(node.state / "cli.jsonl")
                      if entry["source"] == "agy" and entry["argv"][-1:] == ["update"]][-1]
            assert update["argv"] == PROFILE_ARGS + ["update"] and update["stdin_closed"], update
            assert update["home"] == update["cwd"] == str(node.root / "home"), update
            assert update["marker"] == "private-agy-profile", update
        finally:
            flag.unlink(missing_ok=True)
    page.locator("#settings-dialog .modal-close").click()
    passed(machine + ": official manifest, --version, clicked update success/failure, closed stdin and profile env/args")


def check_missing(page, node, *, node_id=None):
    phase("Hub missing Agy gate" if node_id else "local missing Agy gate")
    creates = []
    def on_request(request):
        if urlsplit(request.url).path.endswith("/api/term/create"):
            creates.append(request.url)
    page.on("request", on_request)
    try:
        open_dialog(page)
        if node_id:
            page.locator("#new-node").select_option(node_id)
        radio = page.locator('input[name="new-source"][value="agy"]')
        expect(radio).to_be_disabled()
        expect(radio).not_to_be_checked()
        label = page.locator('#new-session-form label:has(input[value="agy"])')
        assert "未安装" in (label.get_attribute("title") or "")
        box = label.bounding_box()
        assert box
        page.mouse.click(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
        expect(radio).not_to_be_checked()
        expect(page.locator('input[name="new-source"][value="codex"]')).to_be_enabled()
        assert not creates, creates
        page.locator("#new-session-dialog .modal-cancel").click()
        invocations = records(node.state / "cli.jsonl")
        missing = [entry for entry in invocations if entry["source"] == "missing-agy"]
        assert missing and all(entry["argv"] in (PROFILE_ARGS + ["--version"], PROFILE_ARGS + ["models"])
                               for entry in missing), missing
        assert not any(entry["url"] == MANIFEST_URL for entry in records(node.state / "curl.jsonl")), \
            "missing Agy must not fetch a manifest"
        passed(("Hub" if node_id else "local") + ": actual status-127 probe disables Agy; Codex stays available")
    finally:
        page.remove_listener("request", on_request)


def run(args):
    if sys.platform != "linux" or os.uname().machine != "x86_64":
        raise AssertionError("This manifest fixture requires Linux amd64")
    for name in ("binary", "hub_binary", "ptyhost"):
        setattr(args, name, getattr(args, name).resolve(strict=True))
    # Browser assets may be read from the installed cache; all subprocess HOME
    # and XDG directories remain private, including the browser and Hub.
    browser_cache = os.environ.get("PLAYWRIGHT_BROWSERS_PATH", str(
        Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "ms-playwright"))
    chromium = os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE")
    with tempfile.TemporaryDirectory(prefix="sessiondock-agy-clients-") as temporary:
        root = Path(temporary).resolve()
        (root / "home").mkdir(mode=0o700)
        (root / "bin").mkdir(mode=0o700)
        (root / "hub").mkdir(mode=0o700)
        runner_env = private_env(root)
        runner_env["PLAYWRIGHT_BROWSERS_PATH"] = browser_cache
        runner_env["SESSIONDOCK_TEST_WEB_DIR"] = str(REPO / "legacy-web")
        with patch.dict(os.environ, runner_env, clear=True), sync_playwright() as playwright, ExitStack() as stack:
            options = {"headless": True}
            if chromium:
                options["executable_path"] = chromium
            browser = playwright.chromium.launch(**options)
            stack.callback(browser.close)
            installed = stack.enter_context(node_fixture(root / "installed", args, "AgyNode", installed=True))
            missing_stack = stack.enter_context(ExitStack())
            missing = missing_stack.enter_context(node_fixture(root / "missing", args, "MissingNode", installed=False))
            with page_fixture(browser, installed.base) as page:
                check_picker(page, installed, levels=("", *EFFORTS))
                check_updates(page, installed, "本机")
            with page_fixture(browser, missing.base) as page:
                check_missing(page, missing)
                open_machines(page)
                expect(page.locator("#client-matrix thead th")).to_have_text(["机器", "Codex"])
                expect(page.locator('td[data-client-source="agy"]')).to_have_count(0)
                passed("local matrix hides Agy when no machine has it installed")
            phase("start private Hub")
            hub = Hub(args.hub_binary, root / "hub", [installed, missing])
            stack.callback(hub.stop)
            hub.start()
            base = f"http://127.0.0.1:{hub.port}"
            with page_fixture(browser, base) as page:
                nodes_response = page.request.get(base + "/api/nodes")
                assert nodes_response.status == 200, nodes_response.text()[:1200]
                nodes = {n["name"]: n["id"] for n in nodes_response.json()["nodes"]}
                assert set(nodes) == {"AgyNode", "MissingNode"}, nodes
                check_picker(page, installed, node_id=nodes["AgyNode"], levels=("", "high"))
                check_missing(page, missing, node_id=nodes["MissingNode"])
                check_updates(page, installed, "AgyNode", node_id=nodes["AgyNode"])
                open_machines(page)
                row = matrix_row(page, "MissingNode")
                expect(row.locator('td[data-client-source="agy"]')).to_have_text("无")
                expect(row.locator('td[data-client-source="agy"] .client-update')).to_have_count(0)
                expect(row.locator('td[data-client-source="codex"] .client-version')).to_have_text("0.159.0")
                passed("Hub matrix shows Agy only for installed node; missing cell has no update button")
                phase("Hub offline matrix")
                missing_stack.close()  # Only the synthetic node owned by this invocation.
                page.reload(wait_until="networkidle")
                open_machines(page)
                expect(matrix_row(page, "MissingNode").locator("td.client-status")).to_have_text("离线", timeout=20000)
                expect(matrix_row(page, "AgyNode").locator('td[data-client-source="agy"] .client-version')).to_have_text(NEW)
                passed("Hub offline node reads 离线 while the installed Agy node remains usable")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--hub-binary", type=Path)
    parser.add_argument("--ptyhost", type=Path, default=REPO / "target/debug/ptyhost")
    parser.add_argument("--report", type=Path, default=Path("/tmp/sessiondock-agy-clients-report.md"))
    args = parser.parse_args()
    args.hub_binary = args.hub_binary or args.binary.parent / "sessiondock-hub"
    started = time.monotonic()
    failure = None
    try:
        run(args)
    except BaseException as error:
        failure = f"{type(error).__name__}: {error}"
        print("FAIL " + PHASE + ": " + failure[:3000], flush=True)
        traceback.print_exc(limit=5)
    finally:
        report = ["# Agy clients Chromium report", "",
                  "Result: " + ("FAIL" if failure else "PASS"),
                  f"Machine: {os.uname().nodename}; elapsed: {time.monotonic() - started:.1f}s",
                  "Frontend: legacy; real loopback Rust nodes and Hub; private fake CLI/curl; no unit tests.", "",
                  *["- PASS: " + name for name in CHECKS]]
        if failure:
            report.extend(["", "Failed phase: " + PHASE, "", "```text", failure[:5000], "```"])
        report.extend(["", "Scope: models/version/update and missing/offline gates only; this does not claim completion of the overall Agy integration."])
        args.report.write_text("\n".join(report) + "\n")
        print("Report: " + str(args.report), flush=True)
    return 1 if failure else 0


if __name__ == "__main__":
    raise SystemExit(main())
