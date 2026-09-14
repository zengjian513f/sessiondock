#!/usr/bin/env python3
# run_validation: skip
"""Console keystroke→echo latency and burst throughput in headless Chromium.

One isolated ptyhost runs a fixed shell loop with tty echo on, so every
keystroke is echoed by the kernel line discipline without any application
in the loop. The browser sends each byte on the attached WebSocket and waits
for xterm to parse the echoed character (`onWriteParsed`), which covers the
whole output path: ptyhost → sessiondock → WebSocket → term.js batching →
xterm parser. `burst` times `seq 1 N` from Enter to the trailing marker.

`--modes` overrides term.js's `writeTermOutput` in the page so batching
policies compare on one server: `served` (the file as shipped), `timer`
(the former 20 ms setTimeout merge), `frame` (requestAnimationFrame), `none`
(write straight into xterm).

Usage: python3 tests/bench_term_echo_browser.py [--keys 200] [--burst 30000] [--rounds 2] [--modes served,none]
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import tempfile
import time
import uuid

from playwright.sync_api import expect, sync_playwright

from history_parity import BINARY, REPO, Corpus, codex_message, codex_row, isolated_server
from host_identity import request as host_request
from terminal_browser import stop

SHELL = """stty echo
printf 'RS_SHELL_READY\\n'
while IFS= read -r command; do
  case "$command" in
    burst*) seq 1 "${command#burst }"; printf 'RS_BURST_DONE\\n' ;;
    quit) exit 0 ;;
  esac
done
"""

XTERM_TEXT = """() => [...T.views.values()].map(view => {
  const buffer = view.term?.buffer?.active;
  return buffer ? Array.from({length:buffer.length}, (_,i) =>
    buffer.getLine(i)?.translateToString(true) || '').join('\\n').trimEnd() : '';
}).join('\\n')"""

# Each sample: one byte on the WebSocket, resolved when the echoed character
# is parsed into the cursor line. Enter every 40 characters keeps the line
# short; its completion is the cursor returning to column 0 on an empty line.
ECHO_BENCH = """async count => {
  const view = [...T.views.values()][0];
  const term = view.term, ws = view.ws, enc = new TextEncoder();
  const line = () => {
    const buffer = term.buffer.active;
    return buffer.getLine(buffer.baseY + buffer.cursorY)?.translateToString(true) || '';
  };
  const waitFor = pred => new Promise((resolve, reject) => {
    const timer = setTimeout(() => { sub.dispose(); reject(new Error('echo timeout: ' + line())); }, 5000);
    const sub = term.onWriteParsed(() => {
      if (!pred()) return;
      clearTimeout(timer); sub.dispose(); resolve();
    });
  });
  const samples = [];
  let typed = '';
  for (let i = 0; i < count; i++) {
    if (typed.length >= 40) {
      ws.send(enc.encode('\\r'));
      await waitFor(() => term.buffer.active.cursorX === 0 && line() === '');
      typed = '';
    }
    const c = String.fromCharCode(97 + (i % 26));
    typed += c;
    const started = performance.now();
    ws.send(enc.encode(c));
    await waitFor(() => line().endsWith(typed));
    samples.push(performance.now() - started);
  }
  ws.send(enc.encode('\\r'));
  await waitFor(() => term.buffer.active.cursorX === 0 && line() === '');
  return samples;
}"""

BATCH_MODE = """mode => {
  if (mode === 'served') return;
  const flush = view => {
    const s = view.benchBuffer || '';
    view.benchBuffer = ''; view.benchTimer = null;
    if (s) view.term.write(terminalColorChunk(view, s));
  };
  const queue = schedule => (view, chunk) => {
    if (!chunk) return;
    view.benchBuffer = (view.benchBuffer || '') + chunk;
    if (view.benchBuffer.length >= 32 * 1024) return flush(view);
    if (!view.benchTimer) view.benchTimer = schedule(() => flush(view));
  };
  const policies = {
    none: (view, chunk) => { if (chunk) view.term.write(terminalColorChunk(view, chunk)); },
    timer: queue(fn => setTimeout(fn, 20)),
    frame: queue(fn => requestAnimationFrame(fn)),
  };
  if (!policies[mode]) throw new Error('unknown batch mode ' + mode);
  globalThis.writeTermOutput = policies[mode];
}"""

BURST_BENCH = """async lines => {
  const view = [...T.views.values()][0];
  const term = view.term, ws = view.ws, enc = new TextEncoder();
  const buffer = term.buffer.active;
  const tail = () => buffer.getLine(buffer.baseY + buffer.cursorY - 1)?.translateToString(true) || '';
  const done = new Promise((resolve, reject) => {
    const timer = setTimeout(() => { sub.dispose(); reject(new Error('burst timeout: ' + tail())); }, 60000);
    const sub = term.onWriteParsed(() => {
      if (!tail().startsWith('RS_BURST_DONE')) return;
      clearTimeout(timer); sub.dispose(); resolve();
    });
  });
  const started = performance.now();
  ws.send(enc.encode('burst ' + lines + '\\r'));
  await done;
  return performance.now() - started;
}"""


def host(root, instance, uid):
    name = "bench-echo-host"
    environment = {key: value for key, value in os.environ.items() if key in {"PATH", "LANG", "LC_ALL", "LC_CTYPE"}}
    environment["TERM"] = "xterm-256color"
    metadata = {"source": "codex", "sid": "synthetic-native-sid", "uid": uid, "instance_id": instance}
    process = subprocess.Popen([str(REPO / "target/debug/ptyhost"), "--dir", str(root / "host"), "run", "--name", name,
        "--cwd", str(root / "work"), "--cols", "120", "--rows", "40", "--meta", json.dumps(metadata), "--",
        shutil.which("sh"), "-c", SHELL], cwd=root / "work", env=environment,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    deadline = time.monotonic() + 5
    path = root / "host" / (name + ".json")
    while time.monotonic() < deadline:
        assert process.poll() is None, "synthetic host exited early"
        if path.is_file():
            record = json.loads(path.read_text())
            if record["host_pid"] == process.pid:
                try:
                    if "RS_SHELL_READY" in host_request(record, {"op": "capture", "styled": False}).get("text", ""):
                        return process
                except (OSError, json.JSONDecodeError):
                    pass
        time.sleep(.03)
    stop(process)
    raise AssertionError("isolated shell startup timeout")


def open_console(page, uid):
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    expect(page.locator("#a-term")).to_be_visible()
    if not page.locator("#termpane").is_visible():
        page.locator("#a-term").click()
    expect(page.locator("#termpane")).to_be_visible()
    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
    page.wait_for_function("needle => (" + XTERM_TEXT + ")().includes(needle)", arg="RS_SHELL_READY")


def percentile(values, p):
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, round(p * (len(ordered) - 1)))]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--keys", type=int, default=200)
    parser.add_argument("--burst", type=int, default=30000)
    parser.add_argument("--rounds", type=int, default=2)
    parser.add_argument("--modes", default="served")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-benchecho-") as temporary:
        root = Path(temporary)
        for name in ["host", "work", "claude", "codex", "grok"]:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        sid = "synthetic-native-sid"
        corpus.put(sid, "codex", [codex_row("session_meta", {"id": sid, "cwd": str(root / "work")}),
                                  codex_message("user", "Console echo benchmark")], [])
        uid = corpus.uid(sid)
        instance = "synthetic-" + uuid.uuid4().hex
        process = host(root, instance, uid)
        try:
            with isolated_server(corpus, BINARY, host_dir=root / "host") as (base, _), sync_playwright() as playwright:
                launch = {"headless": True}
                if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                    launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
                browser = playwright.chromium.launch(**launch)
                try:
                    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                    page = context.new_page()
                    errors = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(base, wait_until="networkidle")
                    open_console(page, uid)
                    print("renderer=" + page.evaluate("[...T.views.values()][0].renderer"))
                    for mode in args.modes.split(","):
                        page.evaluate(BATCH_MODE, mode)
                        for round_index in range(args.rounds):
                            samples = page.evaluate(ECHO_BENCH, args.keys)
                            print(f"{mode:6} round {round_index + 1} echo n={len(samples)} "
                                  f"p50={percentile(samples, .5):.1f}ms p90={percentile(samples, .9):.1f}ms "
                                  f"p99={percentile(samples, .99):.1f}ms max={max(samples):.1f}ms "
                                  f"mean={statistics.fmean(samples):.1f}ms")
                            elapsed = page.evaluate(BURST_BENCH, args.burst)
                            print(f"{mode:6} round {round_index + 1} burst lines={args.burst} {elapsed:.0f}ms")
                    page.evaluate("[...T.views.values()][0].ws.send(new TextEncoder().encode('quit\\r'))")
                    assert not errors, errors
                finally:
                    browser.close()
        finally:
            stop(process)


if __name__ == "__main__":
    main()
