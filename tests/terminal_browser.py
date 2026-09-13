"""Free, isolated POSIX shell + browser WebSocket acceptance.

Build sessiondock and ptyhost first. This intentionally creates one disposable
PTY running a fixed, noninteractive shell command loop (never Claude/Codex/Grok).
All directories are temporary and every ptyhost invocation supplies --dir. No
native home, production endpoint, host discovery, or paid CLI is used.
"""

import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time
import uuid
from urllib.request import urlopen

from playwright.sync_api import expect, sync_playwright


REPO = Path(__file__).resolve().parents[1]
SHELL_SCRIPT = """stty -echo
printf 'RS_SHELL_READY\\n'
while IFS= read -r command; do
  case "$command" in
    ping) printf 'RS_PING_OK\\n' ;;
    size) stty size ;;
    next) printf 'RS_AFTER_RESTART\\n' ;;
    quit) printf 'RS_SHELL_DONE\\n'; exit 0 ;;
    *) printf 'RS_UNKNOWN_INPUT\\n' ;;
  esac
done
"""

INSTALL = """() => {
  window.__transport = {};
  window.__connectTransport = async (key, page, name, force = false) => {
    const response = await fetch('/api/term/claim', {method:'POST',
      headers:{'Content-Type':'application/json'},
      body:JSON.stringify({name,page,force,_page_id:page,_build:'synthetic',_trace_id:'synthetic'})});
    const claim = await response.json();
    if (!response.ok) return {status:response.status, conflict:claim.conflict === true};
    const url = new URL('/api/term/attach', location.href);
    url.protocol = 'ws:';
    url.search = new URLSearchParams({name,page,token:claim.token,cols:'80',rows:'24',connection:'synthetic-browser'});
    const state = {text:'', notices:[], close:null, terminal:null, ws:new WebSocket(url)};
    state.ws.binaryType = 'arraybuffer';
    window.__transport[key] = state;
    const pre = document.createElement('pre');
    pre.id = 'transport-' + key;
    document.body.append(pre);
    // Exercise the same imported xterm renderer without opening the still-gated
    // legacy CLI/UID console actions. This is a transport-only development test.
    const mount = document.createElement('div');
    mount.style.cssText = 'width:800px;height:260px;position:fixed;left:0;bottom:0;background:#111;z-index:99999';
    document.body.append(mount);
    state.terminal = new Terminal({cols:80,rows:24,allowProposedApi:true});
    state.terminal.open(mount);
    const decoder = new TextDecoder();
    state.ws.onmessage = event => {
      if (typeof event.data === 'string') { state.notices.push(JSON.parse(event.data)); return; }
      const data = new Uint8Array(event.data);
      state.text += decoder.decode(data,{stream:true});
      pre.textContent = state.text;
      state.terminal.write(data);
    };
    state.ws.onclose = event => { state.close = {code:event.code,reason:event.reason}; };
    await new Promise((resolve,reject) => {
      state.ws.onopen = resolve;
      state.ws.onerror = () => reject(new Error('synthetic terminal WebSocket failed'));
      setTimeout(() => reject(new Error('synthetic terminal WebSocket open timeout')),5000);
    });
    return {status:response.status};
  };
}"""


def stop(process):
    if process is not None and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=7)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=3)


def healthy(base, process):
    for _ in range(100):
        if process.poll() is not None:
            raise AssertionError("isolated Rust Web service exited during startup")
        try:
            with urlopen(base + "/api/health", timeout=0.5) as response:
                if response.status == 200:
                    return
        except OSError:
            time.sleep(0.05)
    raise AssertionError("isolated Rust health timeout")


def main():
    if os.name != "posix":
        raise SystemExit("This real-shell check currently requires POSIX; fake TCP peer tests remain portable.")
    server_exe, host_exe = REPO / "target/debug/sessiondock", REPO / "target/debug/ptyhost"
    if not server_exe.is_file() or not host_exe.is_file():
        raise SystemExit("Run cargo build -p sessiondock -p ptyhost --locked first")
    shell = shutil.which("sh")
    if not shell:
        raise SystemExit("A POSIX sh is required for the fixed synthetic command loop")
    with tempfile.TemporaryDirectory(prefix="sessiondock-terminal-") as temporary:
        root = Path(temporary)
        host_dir, work = root / "hosts", root / "work"
        host_dir.mkdir()
        work.mkdir()
        # Do not inherit CLI credentials, profile hooks, native source roots,
        # host-directory defaults, or application runtime settings.
        environment = {key: value for key, value in os.environ.items()
                       if key in {"PATH", "LANG", "LC_ALL", "LC_CTYPE", "SYSTEMROOT", "WINDIR"}}
        environment["TERM"] = "xterm-256color"
        name = "rs-synthetic-shell-" + uuid.uuid4().hex[:12]
        host = subprocess.Popen([str(host_exe), "--dir", str(host_dir), "run", "--name", name,
            "--cwd", str(work), "--cols", "80", "--rows", "24", "--", shell, "-c", SHELL_SCRIPT],
            cwd=work, env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        server = None
        try:
            record_path = host_dir / (name + ".json")
            for _ in range(100):
                if host.poll() is not None:
                    raise AssertionError("synthetic PTY host exited during startup")
                if record_path.is_file():
                    break
                time.sleep(0.05)
            record = json.loads(record_path.read_text())
            assert record["host_pid"] == host.pid
            child_pid = record["pid"]
            with socket.socket() as reservation:
                reservation.bind(("127.0.0.1", 0))
                port = reservation.getsockname()[1]
            base = f"http://127.0.0.1:{port}"
            server_environment = {**environment, "SESSIONDOCK_BIND": f"127.0.0.1:{port}",
                "SESSIONDOCK_WEB_DIR": str(REPO / "legacy-web"), "SESSIONDOCK_PTYHOST_DIR": str(host_dir)}

            def start_web():
                process = subprocess.Popen([str(server_exe)], cwd=work, env=server_environment,
                    stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
                healthy(base, process)
                return process

            server = start_web()
            with sync_playwright() as playwright:
                launch = {"headless": True}
                if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                    launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
                browser = playwright.chromium.launch(**launch)
                context = browser.new_context(viewport={"width":1280,"height":900}, service_workers="block")
                first, second = context.new_page(), context.new_page()
                for page in (first, second):
                    page.goto(base, wait_until="networkidle")
                    # A configured host directory enables the transport, not
                    # CLI creation or a native association for this raw host.
                    assert page.evaluate("AgentHubCapabilities.allows('terminal')") is True
                    assert page.evaluate("AgentHubCapabilities.allows('terminal_create')") is False
                    assert page.evaluate("AgentHubCapabilities.allows('outbox')") is False
                    listing = page.evaluate("fetch('/api/term/list').then(response=>response.json())")
                    assert listing["enabled"] is False and listing["sessions"] == []
                    page.evaluate(INSTALL)
                assert first.evaluate("([name]) => __connectTransport('first','synthetic-page-a',name)", [name])["status"] == 200
                expect(first.locator("#transport-first")).to_contain_text("RS_SHELL_READY")
                first.evaluate("__transport.first.ws.send(new TextEncoder().encode('ping\\n'))")
                expect(first.locator("#transport-first")).to_contain_text("RS_PING_OK")
                first.evaluate("""() => { const state=__transport.first;
                    state.ws.send(JSON.stringify({t:'resize',cols:100,rows:40}));
                    state.ws.send(new TextEncoder().encode('size\\n')); }""")
                expect(first.locator("#transport-first")).to_contain_text("40 100")
                first.wait_for_function("Array.from({length:__transport.first.terminal.buffer.active.length},(_,i)=>__transport.first.terminal.buffer.active.getLine(i)?.translateToString()).join('\\n').includes('RS_PING_OK')")
                denied = second.evaluate("([name]) => __connectTransport('denied','synthetic-page-b',name)", [name])
                assert denied == {"status":409,"conflict":True}
                assert second.evaluate("([name]) => __connectTransport('second','synthetic-page-b',name,true)", [name])["status"] == 200
                first.wait_for_function("__transport.first.close?.code === 4001")
                assert first.evaluate("__transport.first.notices.some(value=>value.t==='revoked')")
                second.evaluate("__transport.second.ws.send(new TextEncoder().encode('ping\\n'))")
                expect(second.locator("#transport-second")).to_contain_text("RS_PING_OK")
                stop(server)
                server = None
                second.wait_for_function("__transport.second.close?.code === 1001")
                assert host.poll() is None
                assert json.loads(record_path.read_text())["pid"] == child_pid
                os.kill(child_pid, 0)  # Only the child PID from our own new record.
                server = start_web()
                second.reload(wait_until="networkidle")
                second.evaluate(INSTALL)
                assert second.evaluate("([name]) => __connectTransport('restarted','synthetic-page-c',name)", [name])["status"] == 200
                second.evaluate("__transport.restarted.ws.send(new TextEncoder().encode('next\\n'))")
                expect(second.locator("#transport-restarted")).to_contain_text("RS_AFTER_RESTART")
                assert json.loads(record_path.read_text())["pid"] == child_pid
                second.evaluate("__transport.restarted.ws.send(new TextEncoder().encode('quit\\n'))")
                expect(second.locator("#transport-restarted")).to_contain_text("RS_SHELL_DONE")
                assert host.wait(timeout=7) == 0
                context.close()
                browser.close()
            print("PASS: isolated real shell, browser bytes/xterm, resize, takeover, Web restart preserves PTY")
        finally:
            stop(server)
            if host.poll() is None:
                # Exact freshly-created test host only; never invoke list or use
                # a default directory. Normal success exits the fixed shell via WS.
                subprocess.run([str(host_exe), "--dir", str(host_dir), "kill", name, "--force"],
                    cwd=work, env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                    timeout=5, check=False)
            stop(host)


if __name__ == "__main__":
    main()
