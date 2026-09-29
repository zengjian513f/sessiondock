#!/usr/bin/env python3
"""New-session model and effort picker: each CLI's own list, one steady row,
search above ten models, and the choice reaching the launched CLI's argv.

Isolated server, private homes and a free fake CLI (`fake_model_cli.py`) for
all four agent sources; no real CLI or model is started."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import urllib.request

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, REPO, Corpus, isolated_server

OPENCODE_MODELS = [f"prov/model-{index:02d}" for index in range(12)] + ["prov/zeta-1", "other/deep/nested-2"]
CODEX_CACHE = {"models": [
    {"slug": "gpt-fake-a", "display_name": "GPT Fake A", "visibility": "list",
     "default_reasoning_level": "low", "supported_reasoning_levels": [{"effort": "low"}, {"effort": "high"}]},
    {"slug": "gpt-fake-b", "display_name": "GPT Fake B", "visibility": "list",
     "default_reasoning_level": "low",
     "supported_reasoning_levels": [{"effort": "low"}, {"effort": "medium"}, {"effort": "xhigh"}]},
    {"slug": "gpt-fake-hidden", "display_name": "Hidden", "visibility": "hide",
     "supported_reasoning_levels": [{"effort": "ultra"}]},
]}
GROK_CACHE = {"models": {
    "grok-fake": {"info": {"id": "grok-fake", "name": "Grok Fake", "hidden": False,
        "supports_reasoning_effort": True, "reasoning_efforts": [
            {"value": "high", "default": True}, {"value": "low", "default": False}]}},
    "grok-secret": {"info": {"id": "grok-secret", "name": "Secret", "hidden": True}},
}}


def argv_lines(log):
    return [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []


def wait_argv(log, predicate, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        found = [argv for argv in argv_lines(log) if predicate(argv)]
        if found:
            return found[-1]
        time.sleep(0.1)
    raise AssertionError(f"no matching argv in {argv_lines(log)}")


def open_dialog(page):
    page.locator("#new-session").click()
    expect(page.locator("#new-session-dialog")).to_be_visible()


def pick_source(page, source):
    page.locator(f'#new-session-form label:has(input[name="new-source"][value="{source}"])').click()
    expect(page.locator("#new-model-label")).not_to_have_text("读取模型…")


def choose_model(page, name):
    page.locator("#new-model").click()
    expect(page.locator("#new-model-menu")).to_be_visible()
    page.locator("#new-model-options [role=option]", has_text=name).first.click()
    expect(page.locator("#new-model-menu")).to_be_hidden()


def create(page, cwd):
    page.locator("#new-cwd").fill(str(cwd))
    with page.expect_response(lambda r: r.url.endswith("/api/term/create")) as created:
        page.locator("#new-session-go").click()
    assert created.value.status == 200, created.value.text()
    expect(page.locator("#new-session-dialog")).to_be_hidden()
    return json.loads(created.value.request.post_data)


def row_geometry(page):
    return page.evaluate("""() => ['.new-source', '#new-model', '#new-effort', '#new-cwd', '#new-cwd-options']
        .map(s => { const r = document.querySelector(s).getBoundingClientRect();
          return [Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height)]; })""")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-model-picker-") as temporary:
        root = Path(temporary).resolve()
        for name in ("host", "work", "ledger", "state", "home", "codex-home", "grok-home"):
            (root / name).mkdir(mode=0o700)
        (root / "codex-home/models_cache.json").write_text(json.dumps(CODEX_CACHE))
        (root / "home/.claude").mkdir(mode=0o700)
        (root / "home/.claude/settings.json").write_text(json.dumps({
            "model": "opus", "modelSettings": {"opus": {"effortLevel": "medium"}}}))
        (root / "codex-home/config.toml").write_text('model = "gpt-fake-b"\nmodel_reasoning_effort = "medium"\n[profiles.x]\nmodel = "other"\n')
        (root / "grok-home/models_cache.json").write_text(json.dumps(GROK_CACHE))
        log = root / "argv.jsonl"
        env = {"PATH": "/usr/bin:/bin", "HOME": str(root / "home"), "TERM": "xterm-256color",
               "LANG": "C.UTF-8", "SESSIONDOCK_TEST_ARGV_LOG": str(log),
               "SESSIONDOCK_TEST_MODELS": "\n".join(OPENCODE_MODELS),
               "CODEX_HOME": str(root / "codex-home"), "GROK_HOME": str(root / "grok-home")}
        executable = str(Path(sys.executable).resolve())
        profiles = [{"id": f"{source}-cli-v1", "source": source, "executable": executable,
                     "args": [str(REPO / "tests/fake_model_cli.py")], "env": env}
                    for source in ("claude", "codex", "grok", "opencode")]
        launcher = root / "launcher.json"
        launcher.write_text(json.dumps({"schema": 2, "host_binary": str(REPO / "target/debug/ptyhost"),
                                        "host_dir": str(root / "host"), "adapters": [], "profiles": profiles}))
        launcher.chmod(0o600)
        initialized = subprocess.run([str(args.binary), "--initialize-lifecycle", str(root / "ledger")],
                                     cwd=REPO, env={"PATH": "/usr/bin:/bin"}, capture_output=True, timeout=15)
        assert initialized.returncode == 0, initialized.stderr.decode()
        work = root / "work"
        pw = sync_playwright().start()
        options = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = pw.chromium.launch(**options)
        with isolated_server(Corpus(root), args.binary, host_dir=root / "host", lifecycle_dir=root / "ledger",
                             launcher_config=launcher, state_dir=root / "state") as (base, _):
            context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
            context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
            page = context.new_page()
            errors = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            shots = os.environ.get("SESSIONDOCK_TEST_SHOTS")
            shot = (lambda name: page.screenshot(path=os.path.join(shots, f"model-{name}.png"))) if shots else (lambda name: None)
            page.goto(base, wait_until="networkidle")
            page.wait_for_function("T.listLoaded")

            # ---- Claude: aliases, no search at five rows, effort list, one row on desktop.
            open_dialog(page)
            pick_source(page, "claude")
            expect(page.locator("#new-model-label")).to_have_text("Opus")
            expect(page.locator("#new-effort")).to_have_value("medium")
            tops = page.evaluate("""() => ['.new-source', '#new-model', '.new-effort']
                .map(s => Math.round(document.querySelector(s).getBoundingClientRect().top))""")
            assert len(set(tops)) == 1, tops
            widths = page.evaluate("""() => ['.new-model', '.new-effort']
                .map(s => document.querySelector(s).getBoundingClientRect().width)""")
            assert abs(widths[0] - widths[1]) <= 2, widths
            page.locator("#new-model").click()
            expect(page.locator("#new-model-search")).to_be_hidden()
            expect(page.locator("#new-model-options [role=option]")).to_have_count(4)
            # Escape closes only the menu, not the dialog.
            page.keyboard.press("Escape")
            expect(page.locator("#new-model-menu")).to_be_hidden()
            expect(page.locator("#new-session-dialog")).to_be_visible()
            body = create(page, work)
            assert body["model"] == "opus" and body["effort"] == "medium", body
            open_dialog(page)
            choose_model(page, "Opus")
            expect(page.locator("#new-model-label")).to_have_text("Opus")
            efforts = page.locator("#new-effort option").all_inner_texts()
            assert efforts == ["选择强度", "low", "medium", "high", "xhigh", "max"], efforts
            expect(page.locator("#new-effort")).to_have_value("medium")
            page.locator("#new-effort").select_option("high")
            shot("claude")
            body = create(page, work)
            assert body["model"] == "opus" and body["effort"] == "high", body
            argv = wait_argv(log, lambda a: "--session-id" in a)
            assert argv[-4:] == ["--model", "opus", "--effort", "high"], argv

            # ---- The choice is remembered per source; switching sources never moves the row.
            page.reload(wait_until="networkidle")
            page.wait_for_function("T.listLoaded")
            open_dialog(page)
            expect(page.locator("#new-model-label")).to_have_text("Opus")
            expect(page.locator("#new-effort")).to_have_value("high")
            before = row_geometry(page)
            page.evaluate("""() => { window.__rows = []; const sample = () => {
                window.__rows.push(['.new-source', '#new-model', '#new-effort', '#new-cwd', '#new-cwd-options']
                  .map(s => { const r = document.querySelector(s).getBoundingClientRect();
                    return [Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height)]; }));
                if (!window.__stop) requestAnimationFrame(sample); }; requestAnimationFrame(sample); }""")
            for source in ("opencode", "codex", "grok", "claude"):
                pick_source(page, source)
            frames = page.evaluate("() => { window.__stop = true; return window.__rows; }")
            assert all(frame == before for frame in frames), [f for f in frames if f != before][:3]

            # ---- Codex: its models cache (hidden skipped), config default, per-model efforts.
            pick_source(page, "codex")
            expect(page.locator("#new-model-label")).to_have_text("GPT Fake B")
            efforts = page.locator("#new-effort option").all_inner_texts()
            assert efforts == ["选择强度", "low", "medium", "xhigh"], efforts
            expect(page.locator("#new-effort")).to_have_value("medium")
            body = create(page, work)
            assert body["model"] == "gpt-fake-b" and body["effort"] == "medium", body
            open_dialog(page)
            page.locator("#new-model").click()
            names = page.locator("#new-model-options [role=option]").all_inner_texts()
            assert not any("Hidden" in name for name in names) and len(names) == 2, names
            page.keyboard.press("Escape")
            choose_model(page, "GPT Fake A")
            assert page.locator("#new-effort option").all_inner_texts() == ["选择强度", "low", "high"]
            expect(page.locator("#new-effort")).to_have_value("")
            page.locator("#new-effort").select_option("low")
            body = create(page, work)
            assert body["model"] == "gpt-fake-a" and body["effort"] == "low", body
            argv = wait_argv(log, lambda a: "-m" in a and "gpt-fake-a" in a)
            assert argv[-4:] == ["-m", "gpt-fake-a", "-c", 'model_reasoning_effort="low"'], argv

            # The displayed value is a concrete choice, including after reopening.
            open_dialog(page)
            count = len(argv_lines(log))
            body = create(page, work)
            assert body["effort"] == "low", body
            wait_argv(log, lambda a: a[-4:] == ["-m", "gpt-fake-a", "-c", 'model_reasoning_effort="low"'])
            assert any(a[-4:] == ["-m", "gpt-fake-a", "-c", 'model_reasoning_effort="low"'] for a in argv_lines(log)[count:])

            # The report uses the same catalog, independently of the new-session selection.
            page.locator('[data-report-bug]:visible').first.click()
            expect(page.locator("#bug-report-model-label")).to_have_text("GPT Fake B")
            expect(page.locator("#bug-report-effort")).to_have_value("medium")
            page.locator("#bug-report-dialog .modal-close").click()

            # Without a configured effort, fall back to the model cache on refresh.
            (root / "codex-home/config.toml").write_text('model = "gpt-fake-b"\n[profiles.x]\nmodel_reasoning_effort = "high"\n')
            open_dialog(page)
            expect(page.locator("#new-effort")).to_have_value("low")
            page.keyboard.press("Escape")

            # ---- Grok: hidden models skipped, efforts in order, default marked.
            open_dialog(page)
            pick_source(page, "grok")
            page.locator("#new-model").click()
            expect(page.locator("#new-model-options [role=option]")).to_have_count(1)
            page.keyboard.press("Escape")
            choose_model(page, "Grok Fake")
            assert page.locator("#new-effort option").all_inner_texts() == ["选择强度", "low", "high"]
            expect(page.locator("#new-effort")).to_have_value("high")
            page.locator("#new-effort").select_option("low")
            body = create(page, work)
            argv = wait_argv(log, lambda a: "grok-fake" in a)
            assert argv[-4:] == ["-m", "grok-fake", "--reasoning-effort", "low"], argv

            # ---- OpenCode: more than ten models get a search box; no effort; the
            #      model rides on the pre-created session.
            open_dialog(page)
            pick_source(page, "opencode")
            expect(page.locator("#new-effort")).to_be_disabled()
            page.locator("#new-model").click()
            expect(page.locator("#new-model-search")).to_be_visible()
            expect(page.locator("#new-model-search")).to_be_focused()
            expect(page.locator("#new-model-options [role=option]")).to_have_count(len(OPENCODE_MODELS))
            page.keyboard.type("nested")
            expect(page.locator("#new-model-options [role=option]")).to_have_count(1)
            page.keyboard.press("Enter")
            expect(page.locator("#new-model-label")).to_have_text("other/deep/nested-2")
            shot("opencode")
            body = create(page, work)
            assert body["model"] == "other/deep/nested-2" and "effort" not in body, body
            prepared = wait_argv(log, lambda a: a[:2] == ["api", "session.create"])
            data = json.loads(prepared[prepared.index("--data") + 1])
            assert data["model"] == {"providerID": "other", "id": "deep/nested-2"}, data

            # ---- SSH has no model: both controls stay in place, disabled.
            open_dialog(page)
            page.locator('#new-session-form label:has(input[value="shell"])').click()
            expect(page.locator("#new-model")).to_be_disabled()
            expect(page.locator("#new-effort")).to_be_disabled()

            # ---- Phone: model and effort wrap together below the agents, nothing overflows.
            page.set_viewport_size({"width": 390, "height": 844})
            pick_source(page, "opencode")
            layout = page.evaluate("""() => { const r = s => document.querySelector(s).getBoundingClientRect();
                return {agents: r('.new-source').top, model: r('#new-model').top, effort: r('.new-effort').top,
                        dialog: r('#new-session-dialog').right, choice: r('.new-choice').right,
                        wide: document.documentElement.scrollWidth <= innerWidth}; }""")
            assert layout["model"] > layout["agents"] and abs(layout["model"] - layout["effort"]) < 1, layout
            assert layout["choice"] <= layout["dialog"] and layout["wide"], layout
            page.locator("#new-model").click()
            menu = page.locator("#new-model-menu").bounding_box()
            assert menu["x"] >= 0 and menu["x"] + menu["width"] <= 390, menu
            shot("phone")
            page.keyboard.press("Escape")
            page.keyboard.press("Escape")
            expect(page.locator("#new-session-dialog")).to_be_hidden()

            assert not errors, errors
            context.close()

        # ---- A configured CLI that is not installed (a shell wrapper whose
        #      command is missing, exit 127) cannot be picked.
        profiles[0] = {**profiles[0], "executable": str(Path("/bin/sh").resolve()),
                       "args": ["-c", 'exec "$0" "$@"', "sessiondock-missing-claude-cli"]}
        launcher.write_text(json.dumps({"schema": 2, "host_binary": str(REPO / "target/debug/ptyhost"),
                                        "host_dir": str(root / "host"), "adapters": [], "profiles": profiles}))
        with isolated_server(Corpus(root), args.binary, host_dir=root / "host", lifecycle_dir=root / "ledger",
                             launcher_config=launcher, state_dir=root / "state") as (base, _):
            deadline = time.monotonic() + 20
            while True:
                listed = json.load(urllib.request.urlopen(base + "/api/term/list?force=1", timeout=10))
                if listed["sources"].get("claude") is False or time.monotonic() > deadline:
                    break
                time.sleep(0.2)
            assert listed["sources"] == {"claude": False, "codex": True, "grok": True,
                                         "opencode": True, "shell": True}, listed["sources"]
            assert listed["resume_sources"]["claude"] is False, listed["resume_sources"]
            context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
            context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
            page = context.new_page()
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.goto(base, wait_until="networkidle")
            page.wait_for_function("T.listLoaded")
            open_dialog(page)
            claude = page.locator('input[name="new-source"][value="claude"]')
            expect(claude).to_be_disabled()
            expect(claude).not_to_be_checked()
            assert "未安装" in page.locator('#new-session-form label:has(input[value="claude"])').get_attribute("title")
            for source in ("codex", "grok", "opencode", "shell"):
                expect(page.locator(f'input[name="new-source"][value="{source}"]')).to_be_enabled()
            expect(page.locator('input[name="new-source"]:checked')).to_have_value("codex")
            assert not errors, errors
            context.close()
        browser.close()
        pw.stop()
    print("PASS new_session_model_browser: claude/codex/grok/opencode catalogs, remembered choice, "
          "steady row across sources, search above ten, argv and pre-created session carry the model, phone wrap, "
          "an uninstalled CLI cannot be picked")


if __name__ == "__main__":
    main()
