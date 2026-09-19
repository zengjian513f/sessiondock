#!/usr/bin/env python3
"""Real CLI argument contract in the legacy UI with synthetic fake CLIs only.

A Claude-profile session is created through the real dialog (desktop and
mobile) and its pending console must show the fake CLI's echoed argv with the
server-generated `--session-id`; typing one line makes the fake CLI write a
synthetic native record, after which the page follows the runtime association.
An existing synthetic Codex session is resumed through the existing console
button (`/api/term/takeover`), whose fake CLI echoes `resume <sid>`. Before the
second creation the configured Claude executable is rewritten in place, the way
the Windows Claude installer overwrites `claude.exe` while the service runs;
creation must keep working and launch the rewritten file. A second server
then runs with a host binary from before recordings (a wrapper that rejects
`--no-record` with status 2 and otherwise runs the real ptyhost, the Cetus
shape of 2026-09-19): an agent session must still be created through it. No
model binary, native CLI home or production host is touched.
"""
import hashlib
import json
import shlex
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
from urllib.parse import urlsplit
from playwright.sync_api import sync_playwright, expect
from history_parity import REPO, BINARY, Corpus, codex_row, codex_message, isolated_server

CODEX_SID = "8f3c1d2e-4a5b-4c6d-8e7f-90a1b2c3d4e5"
SETTINGS = "/synthetic/bridge-settings.json"
FREE_SHELL = ('exec /bin/sh -c \'stty -echo 2>/dev/null; printf "RS_SHELL_READY\\n"; '
              'while IFS= read -r line; do case "$line" in quit) exit 0 ;; *) printf "RS_INPUT_OK\\n" ;; esac; done\'\n')
# One argument per line: the browser xterm is narrower than a full command
# line, so an ordered line sequence is the exact argv assertion.
FAKE_CODEX = """#!/bin/sh
printf 'FAKE_CODEX_ARGV_BEGIN\\n'
i=0
for arg in "$@"; do printf 'A%s [%s]\\n' "$i" "$arg"; i=$((i+1)); done
printf 'FAKE_CODEX_ARGV_END %s\\n' "$i"
printf 'FAKE_CODEX_SID_ENV [%s]\\n' "$CODEX_COMPANION_SESSION_ID$CLAUDE_CODE_SESSION_ID$GROK_SESSION_ID$TMUX$CODEX_THREAD_ID$CODEX_SESSION_ID$CLAUDE_PID"
printf 'SERVICE_INHERITED [%s]\\n' "$SESSIONDOCK_TEST_INHERITED"
printf 'SERVICE_OVERRIDE [%s]\\n' "$SESSIONDOCK_TEST_OVERRIDE"
printf 'SERVICE_REMOVE [%s]\\n' "$SESSIONDOCK_TEST_REMOVE"
printf 'SERVICE_HOME [%s]\\n' "$HOME"
printf 'SERVICE_HOST [%s]\\n' "$SESSIONDOCK_SESSION"
service-env-tool
""" + FREE_SHELL
# The fake Claude writes one synthetic record for its assigned session ID only
# after the first input line, like a CLI persisting the first user message.
FAKE_CLAUDE = """#!/bin/sh
printf 'FAKE_CLAUDE_ARGV_BEGIN\\n'
i=0
for arg in "$@"; do printf 'A%s [%s]\\n' "$i" "$arg"; i=$((i+1)); done
printf 'FAKE_CLAUDE_ARGV_END %s\\n' "$i"
printf 'FAKE_CLAUDE_SID_ENV [%s]\\n' "$CLAUDE_CODE_SESSION_ID$CODEX_COMPANION_SESSION_ID$GROK_SESSION_ID$TMUX$CODEX_THREAD_ID$CODEX_SESSION_ID$CLAUDE_PID"
printf 'FAKE_CLAUDE_HOME [%s]\\n' "$HOME"
printf 'SERVICE_WRAPPER [%s]\\n' "$SESSIONDOCK_TEST_WRAPPER"
printf 'SERVICE_PATH [%s]\\n' "$PATH"
printf 'SERVICE_HOST_KIND [%s]\\n' "$SESSIONDOCK_TEST_HOST_KIND"
sid=""
while [ $# -gt 0 ]; do
  if [ "$1" = "--session-id" ]; then sid="$2"; fi
  shift
done
stty -echo 2>/dev/null
printf 'RS_SHELL_READY\\n'
while IFS= read -r line; do
  case "$line" in
    quit) exit 0 ;;
    *)
      if [ -n "$sid" ] && [ ! -f "$SESSIONDOCK_TEST_CLAUDE_ROOT/project-history/$sid.jsonl" ]; then
        mkdir -p "$SESSIONDOCK_TEST_CLAUDE_ROOT/project-history"
        printf '{"type":"user","uuid":"%s-u1","parentUuid":null,"sessionId":"%s","cwd":"%s","timestamp":"2026-09-12T10:00:00Z","isSidechain":false,"message":{"role":"user","content":"%s"}}\\n' "$sid" "$sid" "$(pwd -P)" "$line" > "$SESSIONDOCK_TEST_CLAUDE_ROOT/project-history/$sid.jsonl"
      fi
      printf 'RS_INPUT_OK\\n' ;;
  esac
done
"""


def claude_uid(root, sid):
    path = root / "claude/project-history" / f"{sid}.jsonl"
    return "claude:" + hashlib.sha1(str(path).encode()).hexdigest()[:16]


# Long argv lines wrap at the xterm width; join wrapped rows back into logical
# lines before matching so the exact command line can be asserted as one string.
XTERM_LOGICAL = """() => [...T.views.values()].map(view => {
  const buffer = view.term?.buffer?.active;
  if (!buffer) return '';
  const lines = [];
  for (let i = 0; i < buffer.length; i++) {
    const line = buffer.getLine(i);
    if (!line) continue;
    const text = line.translateToString(false);
    if (line.isWrapped && lines.length) lines[lines.length - 1] += text; else lines.push(text);
  }
  return lines.map(line => line.trimEnd()).join('\\n').trimEnd();
}).join('\\n')"""


def argv_lines(label, args):
    return "\n".join([f"FAKE_{label}_ARGV_BEGIN"] + [f"A{index} [{arg}]" for index, arg in enumerate(args)]
                     + [f"FAKE_{label}_ARGV_END {len(args)}"])


def xterm_includes(page, text):
    try:
        page.wait_for_function("text => (" + XTERM_LOGICAL + ")().includes(text)", arg=text, timeout=15000)
    except Exception:
        print("XTERM_DIAGNOSTIC", json.dumps({"expected": text, "buffer": page.evaluate(XTERM_LOGICAL),
            "views": page.evaluate("[...T.views.keys()]"), "sel": page.evaluate("S.sel")}, ensure_ascii=False))
        raise


def create_claude(page, context, base, work, expect_completion, full_argv=True, wrapper="loaded", host_kind=""):
    # Narrow layouts fold the button into the header "more" menu.
    if not page.locator("#new-session").is_visible():
        page.locator("#header-more-btn").click()
    expect(page.locator("#new-session")).to_be_visible()
    page.locator("#new-session").click()
    page.locator('input[name="new-source"][value="claude"]').check()
    cwd = page.locator("#new-cwd")
    if expect_completion:
        # Completion preserves the entered spelling and does not treat cwd as
        # an authorization root.
        cwd.fill("~/linked")
        option = page.locator("#new-cwd-options [data-cwd-option]", has_text="linked-claude/")
        expect(option).to_be_visible()
        option.click()
        expect(cwd).to_have_value("~/linked-claude/")
        for ordinary_path in [str(work.parent / "host") + "/", str(work) + "/../", "/etc/"]:
            completed = context.request.get(base + "/api/term/complete-dir", params={"path": ordinary_path})
            assert completed.status == 200, completed.text()
        parent_rows = context.request.get(
            base + "/api/term/complete-dir", params={"path": str(work) + "/../"}
        ).json()["directories"]
        assert str(work) + "/../work/" in parent_rows, parent_rows
    else:
        cwd.fill("~/claude-area")
    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/term/create") as created:
        page.locator("#new-session-go").click()
    response = created.value
    assert response.status == 200, response.text()
    receipt = response.json()
    assert receipt["running"] and receipt["launch_kind"] == "new_assigned", receipt
    assert receipt["declared_sid"] and receipt["declared_uid"] is None and receipt["native_binding"] == "unbound", receipt
    assert not any(key in receipt for key in ["uid", "sid", "argv", "env", "token", "port", "sock"])
    # Same request replays the same receipt and the same assigned session ID.
    repeated = context.request.post(base + "/api/term/create", data=response.request.post_data_json)
    assert repeated.status == 200 and repeated.json()["record_id"] == receipt["record_id"]
    assert repeated.json()["declared_sid"] == receipt["declared_sid"]
    expect(page.locator("#termpane")).to_be_hidden()
    page.locator("#a-term").click()
    expect(page.locator("#termpane")).to_be_visible()
    expect(page.locator("#composer")).to_be_hidden()
    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
    if full_argv:
        xterm_includes(page, argv_lines("CLAUDE", ["--settings", SETTINGS, "--session-id", receipt["declared_sid"]]))
        xterm_includes(page, "FAKE_CLAUDE_HOME [/synthetic/claude-home]")
    else:
        # The narrow mobile xterm clips wide rows; the identity rows fit.
        xterm_includes(page, f"A2 [--session-id]\nA3 [{receipt['declared_sid']}]\nFAKE_CLAUDE_ARGV_END 4")
    xterm_includes(page, "FAKE_CLAUDE_SID_ENV []")
    xterm_includes(page, f"SERVICE_WRAPPER [{wrapper}]")
    xterm_includes(page, "SERVICE_PATH [/usr/bin:/bin]")
    xterm_includes(page, f"SERVICE_HOST_KIND [{host_kind}]")
    expect(page.locator(".new-session-wait")).to_have_text("")
    return receipt


def main():
    if os.name != "posix":
        raise SystemExit("Real launch acceptance currently requires POSIX; no Windows/macOS claim.")
    with tempfile.TemporaryDirectory(prefix="sessiondock-lifecycle-cli-") as temporary:
        root = Path(temporary).resolve()
        for name in ["host", "host-stale", "work", "work/claude-area", "work/codex-area", "ledger", "ledger-stale",
                     "bin", "claude", "codex", "grok"]:
            (root / name).mkdir(mode=0o700)
        (root / "work/linked-claude").symlink_to(root / "work/claude-area", target_is_directory=True)
        server_wrapper = root / "bin/server-home"
        # Only this child service receives the synthetic environment. The
        # browser process and the operator's home/model configuration stay intact.
        service_env = {
            "HOME": str(root / "work"), "PATH": str(root / "bin") + ":/usr/bin:/bin",
            "SESSIONDOCK_TEST_INHERITED": "service-value",
            "SESSIONDOCK_TEST_OVERRIDE": "service-value",
            "SESSIONDOCK_TEST_REMOVE": "service-value",
        }
        for key in ["CLAUDE_CODE_SESSION_ID", "CODEX_COMPANION_SESSION_ID", "GROK_SESSION_ID",
                    "CODEX_THREAD_ID", "CODEX_SESSION_ID", "CLAUDE_PID", "TMUX", "SESSIONDOCK_SESSION"]:
            service_env[key] = "stale-parent-identity"
        server_wrapper.write_text(
            "#!/bin/sh\n" + "".join(f"export {key}={shlex.quote(value)}\n" for key, value in service_env.items())
            + "exec " + shlex.quote(str(BINARY)) + ' "$@"\n'
        )
        server_wrapper.chmod(0o700)
        corpus = Corpus(root)
        corpus.put(CODEX_SID, "codex", [codex_row("session_meta", {"id": CODEX_SID, "cwd": str(root / "work/codex-area")}),
            codex_message("user", "Unchanged native history")], [])
        native = corpus.paths[CODEX_SID].read_bytes()
        codex_uid = corpus.uid(CODEX_SID)
        for name, body in [("fake-claude", FAKE_CLAUDE), ("fake-codex", FAKE_CODEX),
                           ("service-env-tool", "#!/bin/sh\nprintf 'SERVICE_PATH_OK\\n'\n"),
                           ("cli-wrapper", '#!/bin/sh\nexport SESSIONDOCK_TEST_WRAPPER=loaded\nexec "$@"\n'),
                           # A host binary from before recordings: it rejects the option
                           # with status 2 like the real one did, then runs the real host.
                           ("stale-host", '#!/bin/sh\nfor arg in "$@"; do case "$arg" in --no-record) '
                            'echo "未知参数: $arg" >&2; exit 2;; esac; done\n'
                            'export SESSIONDOCK_TEST_HOST_KIND=stale\nexec ' + shlex.quote(str(REPO / "target/debug/ptyhost")) + ' "$@"\n')]:
            (root / "bin" / name).write_text(body)
            (root / "bin" / name).chmod(0o700)
        configuration = root / "launcher.json"
        configuration.touch(mode=0o600)
        launcher = {"schema": 2, "host_binary": str(REPO / "target/debug/ptyhost"),
            "host_dir": str(root / "host"), "adapters": [], "profiles": [
                {"id": "claude-cli-v1", "source": "claude", "executable": str(root / "bin/cli-wrapper"),
                 "args": [str(root / "bin/fake-claude"), "--settings", SETTINGS], "new_args": ["--session-id", "{session_id}"],
                 "resume_args": ["--resume", "{sid}"],
                 "env": {"PATH": "/usr/bin:/bin", "HOME": "/synthetic/claude-home", "TERM": "xterm-256color",
                         "SESSIONDOCK_TEST_CLAUDE_ROOT": str(root / "claude")}},
                {"id": "codex-cli-v1", "source": "codex", "executable": str(root / "bin/fake-codex"),
                 "args": ["--enable", "default_mode_request_user_input", "-c", "suppress_unstable_features_warning=true"],
                 "resume_args": ["resume", "{sid}"],
                 "env": {"TERM": "xterm-256color", "SESSIONDOCK_TEST_OVERRIDE": "profile-value"},
                 "env_remove": ["SESSIONDOCK_TEST_REMOVE"]}]}
        configuration.write_text(json.dumps(launcher))
        stale_configuration = root / "launcher-stale.json"
        stale_configuration.touch(mode=0o600)
        stale_configuration.write_text(json.dumps({**launcher, "host_binary": str(root / "bin/stale-host"),
                                                   "host_dir": str(root / "host-stale")}))
        for ledger in ["ledger", "ledger-stale"]:
            initialized = subprocess.run([str(BINARY), "--initialize-lifecycle", str(root / ledger)],
                cwd=REPO, env={"PATH": "/usr/bin:/bin"}, capture_output=True, timeout=15)
            assert initialized.returncode == 0, initialized.stderr.decode()
        with sync_playwright() as playwright:
            options = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**options)
            try:
                with isolated_server(corpus, server_wrapper, host_dir=root / "host", lifecycle_dir=root / "ledger", launcher_config=configuration) as (base, _):
                    errors = []
                    claims = []
                    takeovers = []
                    def watch(context):
                        context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
                        context.on("request", lambda request: claims.append(request.post_data_json)
                            if urlsplit(request.url).path == "/api/term/claim" else None)
                        context.on("request", lambda request: takeovers.append(request.post_data_json)
                            if urlsplit(request.url).path == "/api/term/takeover" else None)
                        page = context.new_page()
                        page.on("pageerror", lambda error: errors.append(str(error)))
                        page.on("dialog", lambda dialog: dialog.accept())
                        page.goto(base, wait_until="networkidle")
                        page.get_by_role("button", name="时间轴", exact=True).click()
                        return page

                    # ---- Desktop: Claude profile through the real dialog.
                    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                    page = watch(context)
                    expect(page.locator("#new-session")).to_be_visible()
                    receipt = create_claude(page, context, base, root / "work", expect_completion=True)
                    meta = json.loads((root / "host" / (receipt["name"] + ".json")).read_text())["meta"]
                    assert meta["sid"] == receipt["declared_sid"] and "uid" not in meta and meta["launch_id"] == receipt["launch_id"], meta
                    # Operator binding is refused for a launch that declared its identity.
                    denied = context.request.post(base + "/api/term/bind", data={"record_id": receipt["record_id"],
                        "instance_id": receipt["instance_id"], "uid": codex_uid, "operator_confirmed": True})
                    assert denied.status == 409, denied.text()
                    # First input makes the fake CLI persist its assigned session; the
                    # runtime catalog then associates the host and the page follows.
                    uid = claude_uid(root, receipt["declared_sid"])
                    page.locator("#termpane .xterm-helper-textarea").press_sequentially("hello")
                    page.locator("#termpane .xterm-helper-textarea").press("Enter")
                    xterm_includes(page, "RS_INPUT_OK")
                    assert (root / "claude/project-history" / (receipt["declared_sid"] + ".jsonl")).is_file()
                    page.wait_for_function("uid => S.sel === uid", arg=uid, timeout=20000)
                    expect(page.locator("#termpane")).to_be_visible()
                    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
                    xterm_includes(page, "FAKE_CLAUDE_ARGV_BEGIN\nA0 [--settings]")
                    expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_be_visible(timeout=20000)
                    expect(page.locator(f'#side .item[data-uid="tmux:{receipt["name"]}"]')).to_have_count(0)
                    listed = context.request.get(base + "/api/term/list").json()
                    row = next(row for row in listed["sessions"] if row["uid"] == uid)
                    assert row["name"] == receipt["name"] and row["instance_id"] == receipt["instance_id"] and row["sid"] == receipt["declared_sid"], row
                    assert listed["resume_sources"] == {"claude": True, "codex": True, "grok": False}, listed
                    assert listed["backends"][0]["name"] == "ptyhost" and listed["backends"][1]["available"] is False

                    # ---- Resume the synthetic Codex session with the existing console button.
                    page.locator(f'#side .item[data-uid="{codex_uid}"]').click()
                    expect(page.locator("#a-term")).to_have_attribute("data-unavailable", "false")
                    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/term/takeover") as taken:
                        page.locator("#a-term").click()
                    assert taken.value.status == 200, taken.value.text()
                    resumed = taken.value.json()
                    assert resumed["action"] == "started" and resumed["launch_kind"] == "resume", resumed
                    assert resumed["declared_sid"] == CODEX_SID and resumed["declared_uid"] == codex_uid, resumed
                    assert takeovers[-1].get("request_id") and "force" not in takeovers[-1], takeovers[-1]
                    expect(page.locator("#termpane")).to_be_visible()
                    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
                    xterm_includes(page, argv_lines("CODEX", ["--enable", "default_mode_request_user_input", "-c",
                        "suppress_unstable_features_warning=true", "resume", CODEX_SID]))
                    xterm_includes(page, "FAKE_CODEX_SID_ENV []")
                    for expected in ["SERVICE_INHERITED [service-value]", "SERVICE_OVERRIDE [profile-value]",
                                     "SERVICE_REMOVE []", f"SERVICE_HOME [{root / 'work'}]",
                                     f"SERVICE_HOST [{resumed['name']}]", "SERVICE_PATH_OK"]:
                        xterm_includes(page, expected)
                    assert claims[-1].get("uid") == codex_uid and claims[-1].get("instance_id") == resumed["instance_id"], claims[-1]
                    assert "record_id" not in claims[-1]
                    meta = json.loads((root / "host" / (resumed["name"] + ".json")).read_text())["meta"]
                    assert meta["sid"] == CODEX_SID and meta["uid"] == codex_uid, meta
                    expect(page.locator(f'#side .item[data-uid="tmux:{resumed["name"]}"]')).to_have_count(0)
                    # Repeated takeovers, including the legacy force flag, reuse
                    # the exact live instance.
                    again = context.request.post(base + "/api/term/takeover", data={"uid": codex_uid, "request_id": "browser-repeat-request"})
                    assert again.status == 200 and again.json()["record_id"] == resumed["record_id"] and again.json()["action"] == "reused", again.text()
                    forced = context.request.post(base + "/api/term/takeover", data={"uid": codex_uid, "request_id": "browser-force-request", "force": True})
                    assert forced.status == 200 and forced.json()["record_id"] == resumed["record_id"] and forced.json()["action"] == "reused", forced.text()
                    tmux = context.request.post(base + "/api/term/backend", data={"backend": "tmux"})
                    assert tmux.status == 400, tmux.text()
                    assert len([path for path in (root / "host").glob("*.json")]) == 2
                    assert not errors, errors
                    context.close()

                    # ---- The configured executable is overwritten in place while the
                    # service runs (different length and mtime, same path), like a CLI
                    # self-update on Windows; the next creation launches the new file.
                    (root / "bin/cli-wrapper").write_text(
                        '#!/bin/sh\nexport SESSIONDOCK_TEST_WRAPPER=updated\n'
                        '# rewritten in place after the service started\nexec "$@"\n')
                    (root / "bin/cli-wrapper").chmod(0o700)

                    # ---- Mobile: create another Claude session; the resumed Codex console
                    # is already linked, so its button toggles without a new takeover.
                    mobile = browser.new_context(viewport={"width": 390, "height": 844}, service_workers="block")
                    page = watch(mobile)
                    second = create_claude(page, mobile, base, root / "work", expect_completion=False, full_argv=False,
                                           wrapper="updated")
                    assert second["declared_sid"] != receipt["declared_sid"]
                    bounds = page.locator("#termpane").bounding_box()
                    assert bounds and bounds["x"] >= 0 and bounds["x"] + bounds["width"] <= 391, bounds
                    page.locator(".mobile-back").first.click()
                    before = len(takeovers)
                    page.locator(f'#side .item[data-uid="{codex_uid}"]').click()
                    expect(page.locator("#a-term")).to_have_attribute("data-unavailable", "false")
                    page.locator("#a-term").click()
                    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
                    xterm_includes(page, f"A4 [resume]\nA5 [{CODEX_SID}]\nFAKE_CODEX_ARGV_END 6")
                    assert len(takeovers) == before, "linked console must not start another resume"
                    assert len([path for path in (root / "host").glob("*.json")]) == 3
                    assert not errors, errors
                    mobile.close()
                    assert corpus.paths[CODEX_SID].read_bytes() == native

                # ---- A host binary from before recordings still creates agent
                # sessions: the launcher probes it and drops `--no-record`.
                with isolated_server(corpus, server_wrapper, host_dir=root / "host-stale", lifecycle_dir=root / "ledger-stale",
                                     launcher_config=stale_configuration) as (base, _):
                    errors = []
                    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                    page = watch(context)
                    expect(page.locator("#new-session")).to_be_visible()
                    stale = create_claude(page, context, base, root / "work", expect_completion=False,
                                          wrapper="updated", host_kind="stale")
                    meta = json.loads((root / "host-stale" / (stale["name"] + ".json")).read_text())["meta"]
                    assert meta["sid"] == stale["declared_sid"] and meta["launch_id"] == stale["launch_id"], meta
                    assert not errors, errors
                    context.close()
            finally:
                browser.close()
                # Cleanup only explicitly created instances in this private
                # fixture, protected by their full immutable launch envelope.
                for path in list((root / "host").glob("*.json")) + list((root / "host-stale").glob("*.json")):
                    record = json.loads(path.read_text())
                    meta = record["meta"]
                    try:
                        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
                            stream.settimeout(2)
                            stream.connect(str(path.with_suffix(".sock")))
                            stream.sendall(json.dumps({"op": "launch_guard_v1", "expected_source": meta["source"],
                                "expected_launch_id": meta["launch_id"], "expected_instance_id": meta["instance_id"],
                                "request": {"op": "kill", "force": True}}).encode() + b"\n")
                    except OSError:
                        pass
                deadline = time.monotonic() + 6
                while (list((root / "host").glob("*.sock")) or list((root / "host-stale").glob("*.sock"))) \
                        and time.monotonic() < deadline:
                    time.sleep(.05)
    print("PASS lifecycle CLI browser: Claude profile --session-id echoed in pending console (desktop+mobile), "
          "declared identity followed after the fake CLI persisted its record, Codex resume via console button "
          "with exact `resume <sid>` argv, inherited service PATH/HOME/custom env, explicit overrides/removals, "
          "stale parent identity removal, executable wrapper, reuse including legacy force, tmux refusal, native bytes unchanged, "
          "agent creation through a host that predates --no-record")


if __name__ == "__main__":
    main()
