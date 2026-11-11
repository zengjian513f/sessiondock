#!/usr/bin/env python3
"""Reusable VoiceInput browser contract across node/Hub, all 4 composers, and context filtering.

Exercises real pointer click (<220ms) vs long-press (>=220ms), WebAudio capture,
16kHz mono WAV encoding, OpenRouter multimodal SSE proxy streaming, mid-sentence
caret insertion, slide-up/Escape/click-abort cancellations, silence detection,
and strict exclusion of tool/process/shell-command DOM cards from voice context.
"""
from __future__ import annotations

import argparse
import base64
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import math
from pathlib import Path
import struct
import tempfile
import threading
import time
from types import SimpleNamespace
import urllib.error
import urllib.request
import wave

from playwright.sync_api import expect, sync_playwright

from bug_report_node_browser import open_report
from history_fixtures import (
    BINARY,
    REPO,
    Corpus,
    batch35_meta,
    codex_message,
    codex_row,
    isolated_server,
)
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
import os
from private_hosts import private_hosts
from sdk_start_stop_fixtures import provider
from send_browser import initialize
from session_global_actions_browser import click_action

EXPECTED_MODELS = [
    "qwen/qwen3.8-omni-flash",
    "google/gemini-3.8-flash",
    "openai/gpt-audio-mini",
    "xiaomi/mimo-v2.6-flash",
]


def write_wav_fixture(path: Path, *, silent: bool = False, seconds: float = 2.0, rate: int = 16000) -> None:
    frames = bytearray()
    total = int(seconds * rate)
    for idx in range(total):
        if silent:
            sample = 0
        else:
            # Continuous 440Hz + 880Hz speech-level waveform so any >=220ms slice has strong RMS.
            t = idx / rate
            val = 0.35 * math.sin(2.0 * math.pi * 440.0 * t) + 0.15 * math.sin(2.0 * math.pi * 880.0 * t)
            sample = max(-32767, min(32767, int(val * 32767)))
        frames += struct.pack("<h", sample)
    with wave.open(str(path), "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(rate)
        wf.writeframes(bytes(frames))


def assert_valid_16k_mono_wav(b64_audio: str) -> None:
    raw = base64.b64decode(b64_audio)
    assert len(raw) > 44, f"WAV payload too short: {len(raw)}"
    assert raw[:4] == b"RIFF" and raw[8:12] == b"WAVE", f"Invalid WAV magic: {raw[:12]!r}"
    channels = struct.unpack_from("<H", raw, 22)[0]
    sample_rate = struct.unpack_from("<I", raw, 24)[0]
    bits = struct.unpack_from("<H", raw, 34)[0]
    data_len = struct.unpack_from("<I", raw, 40)[0]
    assert channels == 1, f"Expected mono WAV, got {channels} channels"
    assert sample_rate == 16000, f"Expected 16000Hz WAV, got {sample_rate}Hz"
    assert bits == 16, f"Expected 16-bit PCM WAV, got {bits}-bit"
    assert data_len > 1600, f"Expected >50ms PCM samples, got {data_len} bytes"


class MockOpenRouter:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.requests: list[dict] = []
        self.mode = "normal"
        self.reply_chunks = ["<transcript>", "SessionDock ", "codex-sdk-compose.js ", "语音输入完成", "</transcript>"]
        self.slow_Started = threading.Event()
        self.slow_release = threading.Event()

        outer = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, fmt, *args):
                return

            def do_POST(self):
                length = int(self.headers.get("Content-Length") or "0")
                raw = self.rfile.read(length)
                payload = json.loads(raw.decode("utf-8"))
                auth = self.headers.get("Authorization", "")
                with outer.lock:
                    outer.requests.append({
                        "path": self.path,
                        "auth": auth,
                        "payload": payload,
                    })
                    mode = outer.mode
                    chunks = list(outer.reply_chunks)

                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream; charset=utf-8")
                self.send_header("Cache-Control", "no-cache")
                self.send_header("Connection", "close")
                self.end_headers()

                def send_sse(obj_or_raw):
                    line = (
                        f"data: {obj_or_raw}\n\n"
                        if isinstance(obj_or_raw, str)
                        else f"data: {json.dumps(obj_or_raw, ensure_ascii=False)}\n\n"
                    ).encode("utf-8")
                    self.wfile.write(line)
                    self.wfile.flush()

                try:
                    if mode == "slow":
                        send_sse({"choices": [{"delta": {"content": "<transcript>慢速流片段"}}]})
                        outer.slow_Started.set()
                        outer.slow_release.wait(timeout=5.0)
                        send_sse({"choices": [{"delta": {"content": "不应完整到达</transcript>"}}]})
                        send_sse("[DONE]")
                    elif mode == "refusal":
                        send_sse({"choices": [{"delta": {"content": "I'm sorry, I cannot hear any audio in this clip."}}]})
                        send_sse("[DONE]")
                    else:
                        for piece in chunks:
                            send_sse({"choices": [{"delta": {"content": piece}}]})
                            time.sleep(0.015)
                        send_sse("[DONE]")
                except (BrokenPipeError, ConnectionResetError):
                    pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.port = self.server.server_address[1]
        self.url = f"http://127.0.0.1:{self.port}/v1/chat/completions"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def clear(self) -> None:
        with self.lock:
            self.requests.clear()
        self.slow_Started.clear()
        self.slow_release.clear()

    def snapshot(self) -> list[dict]:
        with self.lock:
            return list(self.requests)

    def close(self) -> None:
        self.slow_release.set()
        self.server.shutdown()
        self.server.server_close()


def long_press_mic(page, button, *, hold_ms: int = 380, drag_dy: float = 0.0, press_escape: bool = False) -> None:
    expect(button).to_be_visible(timeout=10000)
    expect(button).to_be_enabled(timeout=10000)
    box = button.bounding_box()
    assert box, "Microphone button must be visible with a bounding box"
    cx = box["x"] + box["width"] / 2
    cy = box["y"] + box["height"] / 2
    page.mouse.move(cx, cy)
    page.mouse.down()
    expect(button).to_have_attribute("data-voice-state", "recording", timeout=4000)
    page.wait_for_timeout(hold_ms)
    if drag_dy:
        page.mouse.move(cx, cy + drag_dy, steps=6)
        page.wait_for_timeout(60)
    if press_escape:
        page.keyboard.press("Escape")
        page.wait_for_timeout(60)
    page.mouse.up()


def extract_prompt_text(req: dict) -> str:
    parts = []
    for msg in req["payload"].get("messages", []):
        content = msg.get("content")
        if isinstance(content, str):
            parts.append(content)
        elif isinstance(content, list):
            for block in content:
                if block.get("type") == "text":
                    parts.append(block.get("text", ""))
    return "\n".join(parts)


def extract_audio_b64(req: dict) -> str:
    for msg in req["payload"].get("messages", []):
        content = msg.get("content")
        if isinstance(content, list):
            for block in content:
                if block.get("type") == "input_audio":
                    audio = block.get("input_audio") or {}
                    assert audio.get("format") == "wav", audio
                    return audio.get("data", "")
    raise AssertionError(f"No input_audio block in request: {req}")


def launch_audio_browser(pw, wav_path: Path):
    opts: dict = {
        "headless": True,
        "args": [
            "--use-fake-ui-for-media-stream",
            "--use-fake-device-for-media-stream",
            f"--use-file-for-fake-audio-capture={wav_path}",
        ],
    }
    if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
        opts["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
    return pw.chromium.launch(**opts)


def prepare_conversation_dirs(corpus_root: Path, binary: Path) -> dict:
    state_dir = corpus_root / "state"
    host_dir = corpus_root / "host"
    work_dir = corpus_root / "work"
    ledger_dir = corpus_root / "ledger"
    for d in (state_dir, host_dir, work_dir):
        d.mkdir(parents=True, exist_ok=True)
    initialize("--initialize-lifecycle", ledger_dir, binary.resolve())
    launcher = corpus_root / "launcher.json"
    launcher.write_text(json.dumps({
        "schema": 2,
        "host_binary": str(REPO / "target/debug/ptyhost"),
        "host_dir": str(host_dir),
        "profiles": [{
            "id": "voice-codex",
            "source": "codex",
            "executable": "/bin/sh",
            "args": ["-c", "exit 0"],
        }],
    }))
    return {
        "state_dir": state_dir,
        "host_dir": host_dir,
        "lifecycle_dir": ledger_dir,
        "launcher_config": launcher,
        "file_roots": (work_dir,),
        "file_write_roots": (work_dir,),
    }


def test_capability_gate_disabled(pw, binary: Path, root: Path, wav_path: Path) -> None:
    corpus = Corpus(root / "disabled-corpus")
    corpus.put("disabled-sid", "codex", [
        batch35_meta("disabled-sid"),
        codex_message("user", "测试未启用语音输入"),
        codex_message("assistant", "收到"),
    ], ["测试未启用语音输入", "收到"])
    dirs = prepare_conversation_dirs(corpus.root, binary)
    with private_hosts(corpus.root), isolated_server(
        corpus,
        binary,
        **dirs,
        extra_env={"SESSIONDOCK_VOICE_API_KEY": "off"},
    ) as (base, opener):
        meta = json.loads(opener.open(f"{base}/api/meta", timeout=5).read())
        assert meta["capabilities"]["voice_input"] is False, meta["capabilities"]
        assert meta["capabilities"]["conversation_send"] is True, meta["capabilities"]

        req = urllib.request.Request(
            f"{base}/api/voice/transcribe",
            data=json.dumps({"audio": "UklGRg==", "format": "wav"}).encode("utf-8"),
            headers={"Content-Type": "application/json", "Origin": base},
            method="POST",
        )
        try:
            opener.open(req, timeout=5)
            raise AssertionError("Expected HTTP 501 when voice_input is disabled")
        except urllib.error.HTTPError as err:
            assert err.code == 501, err.code
            body = json.loads(err.read().decode("utf-8"))
            assert body.get("code") == "voice_input_unconfigured", body

        browser = launch_audio_browser(pw, wav_path)
        try:
            context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
            page = context.new_page()
            page.goto(base, wait_until="networkidle")
            page.locator(".item").first.click()
            expect(page.locator("#cinput")).to_be_visible()
            expect(page.locator("#cinput")).to_be_enabled()
            assert page.locator("[data-voice-input]").count() == 0
            context.close()
        finally:
            browser.close()
    print("PASS capability gate (SESSIONDOCK_VOICE_API_KEY=off): hidden mic buttons + HTTP 501", flush=True)


def test_context_filtering_and_gestures(
    pw,
    binary: Path,
    root: Path,
    mock_or: MockOpenRouter,
    tone_wav: Path,
    silent_wav: Path,
    output: Path | None,
) -> None:
    corpus = Corpus(root / "ctx-corpus")
    shell_envelope = (
        "<user_shell_command>\n"
        "<command>echo SECRET_USER_SHELL_CMD_TOKEN</command>\n"
        "<result>\nExit code: 0\nDuration: 0.1 seconds\nOutput:\n"
        "SECRET_USER_SHELL_OUTPUT_TOKEN\n</result>\n"
        "</user_shell_command>"
    )
    corpus.put("voice-ctx", "codex", [
        batch35_meta("voice-ctx"),
        codex_message("user", "请检查 SessionDock 里 codex-sdk-compose.js 的草稿同步", ordinal=1),
        codex_row("response_item", {
            "type": "reasoning",
            "summary": [{"type": "summary_text", "text": "SECRET_THINKING_PROCESS_TOKEN"}],
        }, ordinal=2),
        codex_row("response_item", {
            "type": "function_call",
            "name": "shell",
            "call_id": "call-mid",
            "arguments": json.dumps({"command": "cat SECRET_TOOL_ARG_TOKEN"}),
        }, ordinal=3),
        codex_row("response_item", {
            "type": "function_call_output",
            "call_id": "call-mid",
            "output": "SECRET_TOOL_OUTPUT_TOKEN",
        }, ordinal=4),
        codex_message("assistant", "我已核对 MyTavern 与 SessionDock 的 VoiceInput 热词提取接口", ordinal=5),
        codex_message("user", shell_envelope, ordinal=6),
        codex_message("user", "请继续完成浏览器端回归验证", ordinal=7),
        codex_row("response_item", {
            "type": "function_call",
            "name": "shell",
            "call_id": "call-tail",
            "arguments": json.dumps({"command": "ls SECRET_TAIL_TOOL_TOKEN"}),
        }, ordinal=8),
        codex_row("response_item", {
            "type": "function_call_output",
            "call_id": "call-tail",
            "output": "SECRET_TAIL_TOOL_OUTPUT_TOKEN",
        }, ordinal=9),
    ], [])
    dirs = prepare_conversation_dirs(corpus.root, binary)

    with private_hosts(corpus.root), isolated_server(
        corpus,
        binary,
        **dirs,
        extra_env={
            "SESSIONDOCK_VOICE_API_KEY": "sk-test-voice-key",
            "SESSIONDOCK_VOICE_API_URL": mock_or.url,
        },
    ) as (base, _opener):
        browser = launch_audio_browser(pw, tone_wav)
        try:
            context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
            page = context.new_page()
            errors: list[str] = []
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(base, wait_until="networkidle")

            # Fresh browser must not write voiceModel before explicit selection, and all keys use sessiondock.*
            keys = page.evaluate("() => Object.keys(localStorage)")
            assert all(k.startswith("sessiondock.") for k in keys), keys
            assert "sessiondock.voiceModel" not in keys, keys

            page.locator(".item").first.click()
            cinput = page.locator("#cinput")
            expect(cinput).to_be_visible()
            mic = page.locator("#composer [data-voice-input]")
            expect(mic).to_be_visible()
            expect(mic).to_have_attribute("data-voice-model", EXPECTED_MODELS[0])

            # Single-click (<220ms) opens 4-model menu
            mic.click()
            menu = page.locator(".voice-model-menu:popover-open")
            expect(menu).to_be_visible()
            options = menu.locator(".voice-model-option")
            expect(options).to_have_count(4)
            model_ids = options.evaluate_all("els => els.map(e => e.dataset.modelId)")
            assert model_ids == EXPECTED_MODELS, model_ids
            expect(options.nth(0)).to_have_attribute("aria-checked", "true")

            if output:
                page.screenshot(path=str(output / "node-1280-light-composer-menu.png"), full_page=True)

            # Select Gemini 3.8 Flash; verify persistence and cross-composer sync
            options.nth(1).click()
            expect(page.locator(".voice-model-menu:popover-open")).to_have_count(0)
            saved_raw = page.evaluate("() => localStorage.getItem('sessiondock.voiceModel')")
            assert json.loads(saved_raw) == "google/gemini-3.8-flash", saved_raw
            for idx in range(page.locator("[data-voice-input]").count()):
                expect(page.locator("[data-voice-input]").nth(idx)).to_have_attribute(
                    "data-voice-model", "google/gemini-3.8-flash"
                )

            # Expand folded turn-process AND tool group to prove even expanded process/tool DOM is excluded!
            if page.locator("#msgs .turn-process .fold-toggle").count():
                page.locator("#msgs .turn-process .fold-toggle").first.click()
                expect(page.locator("#msgs .turn-process-body")).to_be_visible()

            # Place caret mid-sentence in #cinput
            cinput.fill("开头： [结尾]")
            cinput.focus()
            cinput.evaluate("el => el.setSelectionRange(3, 3)")

            # Capture active recording pill screenshot during a real hold
            mock_or.clear()
            box = mic.bounding_box()
            assert box
            page.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
            page.mouse.down()
            expect(mic).to_have_attribute("data-voice-state", "recording", timeout=4000)
            expect(page.locator(".voice-status-pill:popover-open")).to_be_visible()
            page.wait_for_timeout(360)
            if output:
                page.screenshot(path=str(output / "node-1280-light-recording-pill.png"), full_page=True)
            page.mouse.up()

            expect(cinput).to_have_value("开头：SessionDock codex-sdk-compose.js 语音输入完成 [结尾]", timeout=8000)
            expect(mic).to_have_attribute("data-voice-state", "idle", timeout=5000)

            reqs = mock_or.snapshot()
            assert len(reqs) == 1, len(reqs)
            assert reqs[0]["auth"] == "Bearer sk-test-voice-key"
            assert reqs[0]["payload"]["model"] == "google/gemini-3.8-flash"
            assert reqs[0]["payload"]["stream"] is True
            assert_valid_16k_mono_wav(extract_audio_b64(reqs[0]))

            prompt_text = extract_prompt_text(reqs[0])
            assert "请检查 SessionDock 里 codex-sdk-compose.js 的草稿同步" in prompt_text, prompt_text
            assert "我已核对 MyTavern 与 SessionDock 的 VoiceInput 热词提取接口" in prompt_text, prompt_text
            assert "开头：" in prompt_text and "[结尾]" in prompt_text, prompt_text
            for forbidden in (
                "SECRET_USER_SHELL_CMD_TOKEN",
                "SECRET_USER_SHELL_OUTPUT_TOKEN",
                "SECRET_THINKING_PROCESS_TOKEN",
                "SECRET_TOOL_ARG_TOKEN",
                "SECRET_TOOL_OUTPUT_TOKEN",
                "SECRET_TAIL_TOOL_TOKEN",
                "SECRET_TAIL_TOOL_OUTPUT_TOKEN",
            ):
                assert forbidden not in prompt_text, f"Forbidden token {forbidden} leaked into voice prompt:\n{prompt_text}"

            # Slide-up cancel discards recording without calling OpenRouter
            mock_or.clear()
            before_val = cinput.input_value()
            long_press_mic(page, mic, hold_ms=300, drag_dy=-75)
            expect(mic).to_have_attribute("data-voice-state", "idle")
            page.wait_for_timeout(200)
            assert len(mock_or.snapshot()) == 0
            assert cinput.input_value() == before_val

            # Escape cancel discards recording without calling OpenRouter
            long_press_mic(page, mic, hold_ms=300, press_escape=True)
            expect(mic).to_have_attribute("data-voice-state", "idle")
            page.wait_for_timeout(200)
            assert len(mock_or.snapshot()) == 0
            assert cinput.input_value() == before_val

            # Slow SSE stream abort on click
            mock_or.clear()
            mock_or.mode = "slow"
            cinput.fill("")
            box = mic.bounding_box()
            assert box
            page.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
            page.mouse.down()
            expect(mic).to_have_attribute("data-voice-state", "recording", timeout=4000)
            page.wait_for_timeout(340)
            page.mouse.up()
            assert mock_or.slow_Started.wait(timeout=5.0), "Expected slow OpenRouter stream to start"
            expect(mic).to_have_attribute("data-voice-state", "transcribing", timeout=4000)
            mic.click()
            mock_or.slow_release.set()
            expect(mic).to_have_attribute("data-voice-state", "idle", timeout=4000)
            assert "不应完整到达" not in cinput.input_value()
            mock_or.mode = "normal"

            # Refusal filter suppresses chatty model refusal when no <transcript> tags exist
            mock_or.clear()
            mock_or.mode = "refusal"
            cinput.fill("保留原稿")
            long_press_mic(page, mic, hold_ms=340)
            expect(mic).to_have_attribute("data-voice-state", "idle", timeout=5000)
            assert cinput.input_value() == "保留原稿"
            mock_or.mode = "normal"

            assert not errors, errors
            context.close()
        finally:
            browser.close()

        # Silence guard with pure-zero WAV capture
        silent_browser = launch_audio_browser(pw, silent_wav)
        try:
            mock_or.clear()
            context = silent_browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
            page = context.new_page()
            page.goto(base, wait_until="networkidle")
            page.locator(".item").first.click()
            mic = page.locator("#composer [data-voice-input]")
            expect(mic).to_be_visible()
            long_press_mic(page, mic, hold_ms=360)
            expect(mic).to_have_attribute("data-voice-state", "idle")
            expect(page.locator(".voice-status-pill:popover-open")).to_contain_text("未检测到语音")
            assert len(mock_or.snapshot()) == 0
            context.close()
        finally:
            silent_browser.close()

    print("PASS context filtering (user/assistant included; tool/process/shell excluded), WAV 16kHz, cancel/abort/refusal/silence", flush=True)


def exercise_sdk_dialogs_and_hub(
    browser,
    url: str,
    label: str,
    width: int,
    mock_or: MockOpenRouter,
    output: Path | None,
) -> None:
    dark = width < 720
    context = browser.new_context(
        viewport={"width": width, "height": 860},
        color_scheme="dark" if dark else "light",
        service_workers="block",
    )
    page = context.new_page()
    errors: list[str] = []
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.route("**/api/quotas*", lambda r: r.fulfill(json={"agents": []}))
    page.goto(url, wait_until="networkidle")

    # Select first Codex session -> opens SDK view (#sdk-composer / #sdk-input)
    page.locator(".item").first.click()
    sdk_input = page.locator("#sdk-input")
    expect(sdk_input).to_be_visible(timeout=10000)
    # Wait until SDK view finishes connecting (or enable draft check: look at #sdk-composer mic button)
    sdk_mic = page.locator("#sdk-composer [data-voice-input]")
    expect(sdk_mic).to_be_visible()
    expect(sdk_input).to_be_enabled(timeout=10000)
    expect(sdk_mic).to_be_enabled(timeout=10000)

    # Single-click on SDK mic -> choose GPT Audio Mini on node, MiMo v2.6 Flash on hub
    chosen_idx = 2 if label == "node" else 3
    chosen_model = EXPECTED_MODELS[chosen_idx]
    sdk_mic.click()
    menu = page.locator(".voice-model-menu:popover-open")
    expect(menu).to_be_visible()
    if output:
        page.screenshot(path=str(output / f"{label}-{width}-{'dark' if dark else 'light'}-sdk-menu.png"), full_page=True)
    menu.locator(".voice-model-option").nth(chosen_idx).click()
    expect(sdk_mic).to_have_attribute("data-voice-model", chosen_model)

    # Dictate into #sdk-input (on Hub this proves Hub -> Node SSE proxy with Hub lacking OPENROUTER_API_KEY!)
    sdk_input.fill("")
    mock_or.clear()
    mock_or.reply_chunks = ["<transcript>", f"{label.upper()} SDK 语音测试通过", "</transcript>"]
    long_press_mic(page, sdk_mic, hold_ms=360)
    expect(sdk_input).to_have_value(f"{label.upper()} SDK 语音测试通过", timeout=8000)
    expect(sdk_mic).to_have_attribute("data-voice-state", "idle", timeout=5000)
    reqs = mock_or.snapshot()
    assert len(reqs) == 1, len(reqs)
    assert reqs[0]["payload"]["model"] == chosen_model

    if output:
        page.screenshot(path=str(output / f"{label}-{width}-{'dark' if dark else 'light'}-sdk-transcribed.png"), full_page=True)
        if label == "node" and width == 390:
            page.evaluate("applyInterfaceScale(125, false)")
            sdk_mic.click()
            expect(page.locator(".voice-model-menu:popover-open")).to_be_visible()
            page.screenshot(path=str(output / "node-390-dark-scale125-sdk-menu.png"), full_page=True)
            sdk_mic.click()
            page.evaluate("applyInterfaceScale(100, false)")

    # Reload and verify model preference persisted under namespace
    page.reload(wait_until="networkidle")
    if width < 720 and not page.locator("#sdk-input").is_visible():
        page.locator(".item").first.click()
    expect(page.locator("#sdk-composer [data-voice-input]")).to_have_attribute("data-voice-model", chosen_model, timeout=10000)

    # Open Continue Session dialog (#continue-session-composer inside #new-session-dialog)
    click_action(page, "[data-continue-session]")
    expect(page.locator("#new-session-dialog")).to_be_visible()
    expect(page.locator("#continue-session-composer")).to_be_visible()
    cont_mic = page.locator("#continue-session-composer [data-voice-input]")
    expect(cont_mic).to_be_visible()
    cont_mic.click()
    expect(page.locator("#new-session-dialog .voice-model-menu:popover-open")).to_be_visible()
    if output and label == "node":
        page.screenshot(path=str(output / f"{label}-{width}-{'dark' if dark else 'light'}-continue-dialog.png"), full_page=True)
    # Closing dialog must auto-close the voice model menu
    page.locator("#new-session-dialog .modal-close").click()
    expect(page.locator("#new-session-dialog")).to_be_hidden()
    expect(page.locator(".voice-model-menu:popover-open")).to_have_count(0)

    # Return to list on narrow mobile before opening header bug-report action
    if width < 720 and page.locator(".mobile-back").is_visible():
        page.locator(".mobile-back").click()

    # Open Bug Report dialog (#bug-report-description inside #bug-report-dialog)
    open_report(page)
    bug_input = page.locator("#bug-report-description")
    bug_mic = page.locator("#bug-report-form [data-voice-input]")
    expect(bug_input).to_be_visible()
    expect(bug_mic).to_be_visible()
    bug_mic.click()
    bug_menu = page.locator("#bug-report-dialog .voice-model-menu:popover-open")
    expect(bug_menu).to_be_visible()
    if output:
        page.screenshot(path=str(output / f"{label}-{width}-{'dark' if dark else 'light'}-bug-report-menu.png"), full_page=True)
    # Clicking an option inside bug-report-dialog must NOT dismiss the dialog!
    bug_menu.locator(".voice-model-option").nth(0).click()
    expect(page.locator("#bug-report-dialog")).to_be_visible()
    expect(bug_mic).to_have_attribute("data-voice-model", EXPECTED_MODELS[0])

    mock_or.clear()
    mock_or.reply_chunks = ["<transcript>", "缺陷报告语音录入正常", "</transcript>"]
    long_press_mic(page, bug_mic, hold_ms=360)
    expect(bug_input).to_have_value("缺陷报告语音录入正常", timeout=8000)
    page.locator("#bug-report-dialog .modal-close").click()

    assert not errors, errors
    context.close()
    print(f"PASS {label} {width}px ({'dark' if dark else 'light'}): SDK + Continue + BugReport composers & preference persistence", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--output", type=Path, default=None)
    args = parser.parse_args()
    if args.output:
        args.output.mkdir(parents=True, exist_ok=True)

    mock_or = MockOpenRouter()
    try:
        with tempfile.TemporaryDirectory(prefix="sd-voice-input-") as tmp, sync_playwright() as pw:
            root = Path(tmp)
            tone_wav = root / "tone.wav"
            silent_wav = root / "silent.wav"
            write_wav_fixture(tone_wav, silent=False)
            write_wav_fixture(silent_wav, silent=True)

            test_capability_gate_disabled(pw, args.binary, root, tone_wav)
            test_context_filtering_and_gestures(pw, args.binary, root, mock_or, tone_wav, silent_wav, args.output)

            sdk_root = root / "sdk-hub"
            sdk_root.mkdir()
            with provider(
                "codex",
                sdk_root,
                args.binary,
                report=True,
                extra_env={
                    "SESSIONDOCK_VOICE_API_KEY": "sk-node-voice-key",
                    "SESSIONDOCK_VOICE_API_URL": mock_or.url,
                },
            ) as (node_url, _, extra):
                hub_root = sdk_root / "hub"
                hub_root.mkdir()
                node_port = int(extra["SESSIONDOCK_NODE_BIND"].rsplit(":", 1)[1])
                node_desc = SimpleNamespace(name="VoiceNode", token=TOKEN, port=node_port)
                hub = Hub(args.binary.resolve().with_name("sessiondock-hub"), hub_root, [node_desc])
                # Force Hub itself to have NO local OpenRouter key so it proxies /api/voice/transcribe SSE to VoiceNode!
                hub.env["SESSIONDOCK_VOICE_API_KEY"] = "off"
                hub.env.pop("OPENROUTER_API_KEY", None)
                hub.env.pop("VOICE_INPUT_API_KEY", None)
                hub.start()

                browser = launch_audio_browser(pw, tone_wav)
                try:
                    for label, url in [("node", node_url), ("hub", f"http://127.0.0.1:{hub.port}")]:
                        for width in (1280, 390):
                            exercise_sdk_dialogs_and_hub(browser, url, label, width, mock_or, args.output)
                finally:
                    browser.close()
                    hub.stop()
    finally:
        mock_or.close()


if __name__ == "__main__":
    main()
