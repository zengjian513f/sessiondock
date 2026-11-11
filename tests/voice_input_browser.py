#!/usr/bin/env python3
"""Reusable 3-provider VoiceInput browser contract (OpenAI, Google AI Studio, OpenRouter).

Exercises:
- Project JSON config (`SESSIONDOCK_VOICE_CONFIG`) + capability gate (`voice_input`, `voice_providers`)
- Single-click (<220ms) 3-provider tabbed model selector (`OpenAI` | `Google` | `OpenRouter`) + `实时` badges
- Real-time streaming dictation while holding (`openai:gpt-live-transcribe`, `google:gemini-3.5-transcribe-live`):
  - WebSocket `/api/voice/realtime` binary 16kHz PCM upload + resampling
  - Dedicated `.voice-live-card` HUD (committed words, dashed `.voice-live-interim`, blinking `.voice-live-caret`, 5-bar meter)
  - Live `<textarea>` insertion BEFORE pointer release ("边说话边出字")
  - Exact `<textarea>` rollback when pressing `Escape` or sliding up mid-dictation
- HTTP/SSE WAV transcription across OpenAI (`/v1/audio/transcriptions`), Google (`streamGenerateContent`), and OpenRouter
- Strict context filtering (includes recent `user`/`assistant` + draft; excludes `.tool-msg`, `.turn-process`, `.user-shell-command`)
- Node & Hub modes (including Hub zero-copy WebSocket `/api/voice/realtime` and SSE `/api/voice/transcribe` forwarding)
"""
from __future__ import annotations

import argparse
import base64
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import math
import os
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
from private_hosts import private_hosts
from sdk_start_stop_fixtures import provider
from send_browser import initialize
from session_global_actions_browser import click_action

EXPECTED_OPENAI_MODELS = [
    "openai:gpt-live-transcribe",
    "openai:gpt-4o-transcribe",
    "openai:gpt-4o-mini-transcribe",
]
EXPECTED_GOOGLE_MODELS = [
    "google:gemini-3.5-transcribe-live",
    "google:gemini-3.8-flash",
    "google:gemini-4-carbon",
]
EXPECTED_OPENROUTER_MODELS = [
    "openrouter:qwen/qwen3.8-omni-flash",
    "openrouter:google/gemini-3.8-flash",
    "openrouter:openai/gpt-audio-mini",
    "openrouter:xiaomi/mimo-v2.6-flash",
]


def write_wav_fixture(path: Path, *, silent: bool = False, seconds: float = 2.4, rate: int = 16000) -> None:
    frames = bytearray()
    total = int(seconds * rate)
    for idx in range(total):
        if silent:
            sample = 0
        else:
            t = idx / rate
            val = 0.35 * math.sin(2.0 * math.pi * 440.0 * t) + 0.15 * math.sin(2.0 * math.pi * 880.0 * t)
            sample = max(-32767, min(32767, int(val * 32767)))
        frames += struct.pack("<h", sample)
    with wave.open(str(path), "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(rate)
        wf.writeframes(bytes(frames))


def assert_valid_16k_mono_wav_bytes(raw: bytes) -> None:
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


def assert_valid_16k_mono_wav(b64_audio: str) -> None:
    assert_valid_16k_mono_wav_bytes(base64.b64decode(b64_audio))


def parse_multipart_fields(raw: bytes, content_type: str) -> dict[str, bytes]:
    m_boundary = None
    for part in content_type.split(";"):
        part = part.strip()
        if part.lower().startswith("boundary="):
            m_boundary = part.split("=", 1)[1].strip('"')
    assert m_boundary, f"Missing multipart boundary in {content_type!r}"
    delim = f"--{m_boundary}".encode("ascii")
    out: dict[str, bytes] = {}
    for seg in raw.split(delim)[1:]:
        if seg.startswith(b"--") or not seg.strip():
            continue
        seg = seg.lstrip(b"\r\n")
        if b"\r\n\r\n" not in seg:
            continue
        head_bytes, body = seg.split(b"\r\n\r\n", 1)
        if body.endswith(b"\r\n"):
            body = body[:-2]
        head_str = head_bytes.decode("utf-8", errors="replace")
        for token in head_str.split(";"):
            token = token.strip()
            if token.startswith('name="') and token.endswith('"'):
                out[token[6:-1]] = body
                break
    return out


def ws_accept_key(sec_key: str) -> str:
    guid = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
    digest = hashlib.sha1((sec_key.strip() + guid).encode("ascii")).digest()
    return base64.b64encode(digest).decode("ascii")


def ws_recv_frame(sock) -> tuple[int, bytes] | None:
    def read_exact(n: int) -> bytes | None:
        buf = bytearray()
        while len(buf) < n:
            chunk = sock.recv(n - len(buf))
            if not chunk:
                return None
            buf.extend(chunk)
        return bytes(buf)

    hdr = read_exact(2)
    if hdr is None:
        return None
    opcode = hdr[0] & 0x0F
    masked = (hdr[1] & 0x80) != 0
    length = hdr[1] & 0x7F
    if length == 126:
        ext = read_exact(2)
        if ext is None:
            return None
        length = struct.unpack(">H", ext)[0]
    elif length == 127:
        ext = read_exact(8)
        if ext is None:
            return None
        length = struct.unpack(">Q", ext)[0]
    mask = read_exact(4) if masked else b""
    if masked and mask is None:
        return None
    payload = read_exact(length) if length > 0 else b""
    if payload is None:
        return None
    if masked:
        payload = bytes(b ^ mask[i & 3] for i, b in enumerate(payload))
    return opcode, payload


def ws_send_json(sock, obj: dict) -> None:
    data = json.dumps(obj, ensure_ascii=False).encode("utf-8")
    n = len(data)
    if n < 126:
        hdr = bytes([0x81, n])
    elif n < 65536:
        hdr = bytes([0x81, 126]) + struct.pack(">H", n)
    else:
        hdr = bytes([0x81, 127]) + struct.pack(">Q", n)
    sock.sendall(hdr + data)


class MockVoiceUpstream:
    """Unified loopback mock for OpenAI (HTTP+WS), Google AI Studio (HTTP+WS), and OpenRouter (HTTP)."""

    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.http_requests: list[dict] = []
        self.ws_sessions: list[dict] = []
        self.or_mode = "normal"
        self.or_chunks = ["<transcript>", "OpenRouter ", "语音输入完成", "</transcript>"]
        self.openai_http_deltas = ["OpenAI ", "GPT-4o ", "流式转写完成"]
        self.google_http_chunks = ["<transcript>", "Google ", "Gemini 3.8 Flash ", "转写成功", "</transcript>"]
        self.openai_ws_live_deltas = ["边说话", "边出字"]
        self.openai_ws_final_suffix = "，实时完成！"
        self.google_ws_interim = "正在实时校准热词…"
        self.google_ws_segment = "Gemini Live 实时流式输出"
        self.slow_started = threading.Event()
        self.slow_release = threading.Event()

        outer = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, fmt, *args):
                return

            def do_GET(self):
                if self.headers.get("Upgrade", "").lower() != "websocket":
                    self.send_response(400)
                    self.end_headers()
                    return
                sec_key = self.headers.get("Sec-WebSocket-Key", "")
                auth = self.headers.get("Authorization", "")
                self.send_response(101, "Switching Protocols")
                self.send_header("Upgrade", "websocket")
                self.send_header("Connection", "Upgrade")
                self.send_header("Sec-WebSocket-Accept", ws_accept_key(sec_key))
                self.end_headers()
                self.wfile.flush()
                sock = self.connection
                sock.settimeout(12.0)

                if self.path.startswith("/ws/openai"):
                    outer._serve_openai_ws(sock, self.path, auth)
                elif self.path.startswith("/ws/google"):
                    outer._serve_google_ws(sock, self.path)
                self.close_connection = True

            def do_POST(self):
                length = int(self.headers.get("Content-Length") or "0")
                raw = self.rfile.read(length)
                ctype = self.headers.get("Content-Type", "")
                auth = self.headers.get("Authorization", "")
                goog_key = self.headers.get("x-goog-api-key", "")

                if self.path.startswith("/openai/v1/audio/transcriptions"):
                    fields = parse_multipart_fields(raw, ctype)
                    rec = {
                        "provider": "openai",
                        "path": self.path,
                        "auth": auth,
                        "model": fields.get("model", b"").decode("utf-8"),
                        "stream": fields.get("stream", b"").decode("utf-8"),
                        "prompt": fields.get("prompt", b"").decode("utf-8"),
                        "file": fields.get("file", b""),
                    }
                    with outer.lock:
                        outer.http_requests.append(rec)
                        deltas = list(outer.openai_http_deltas)
                    self._start_sse()
                    try:
                        for d in deltas:
                            self._send_sse({"type": "transcript.text.delta", "delta": d})
                            time.sleep(0.015)
                        self._send_sse({"type": "transcript.text.done", "text": "".join(deltas)})
                        self._send_sse("[DONE]")
                    except (BrokenPipeError, ConnectionResetError):
                        pass
                    return

                if self.path.startswith("/google/v1beta/models/"):
                    payload = json.loads(raw.decode("utf-8"))
                    rec = {
                        "provider": "google",
                        "path": self.path,
                        "api_key": goog_key,
                        "payload": payload,
                    }
                    with outer.lock:
                        outer.http_requests.append(rec)
                        chunks = list(outer.google_http_chunks)
                    self._start_sse()
                    try:
                        for piece in chunks:
                            self._send_sse({
                                "candidates": [{"content": {"parts": [{"text": piece}]}}]
                            })
                            time.sleep(0.015)
                    except (BrokenPipeError, ConnectionResetError):
                        pass
                    return

                payload = json.loads(raw.decode("utf-8"))
                with outer.lock:
                    outer.http_requests.append({
                        "provider": "openrouter",
                        "path": self.path,
                        "auth": auth,
                        "payload": payload,
                    })
                    mode = outer.or_mode
                    chunks = list(outer.or_chunks)

                self._start_sse()
                try:
                    if mode == "slow":
                        self._send_sse({"choices": [{"delta": {"content": "<transcript>慢速流片段"}}]})
                        outer.slow_started.set()
                        outer.slow_release.wait(timeout=5.0)
                        self._send_sse({"choices": [{"delta": {"content": "不应完整到达</transcript>"}}]})
                        self._send_sse("[DONE]")
                    elif mode == "refusal":
                        self._send_sse({"choices": [{"delta": {"content": "I'm sorry, I cannot hear any audio in this clip."}}]})
                        self._send_sse("[DONE]")
                    else:
                        for piece in chunks:
                            self._send_sse({"choices": [{"delta": {"content": piece}}]})
                            time.sleep(0.015)
                        self._send_sse("[DONE]")
                except (BrokenPipeError, ConnectionResetError):
                    pass

            def _start_sse(self):
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream; charset=utf-8")
                self.send_header("Cache-Control", "no-cache")
                self.send_header("Connection", "close")
                self.end_headers()

            def _send_sse(self, obj_or_raw):
                line = (
                    f"data: {obj_or_raw}\n\n"
                    if isinstance(obj_or_raw, str)
                    else f"data: {json.dumps(obj_or_raw, ensure_ascii=False)}\n\n"
                ).encode("utf-8")
                self.wfile.write(line)
                self.wfile.flush()

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.port = self.server.server_address[1]
        self.openrouter_url = f"http://127.0.0.1:{self.port}/openrouter/v1/chat/completions"
        self.openai_transcribe_url = f"http://127.0.0.1:{self.port}/openai/v1/audio/transcriptions"
        self.openai_realtime_url = f"ws://127.0.0.1:{self.port}/ws/openai?intent=transcription"
        self.google_api_base = f"http://127.0.0.1:{self.port}/google"
        self.google_realtime_url = f"ws://127.0.0.1:{self.port}/ws/google"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def _serve_openai_ws(self, sock, path: str, auth: str) -> None:
        record = {
            "provider": "openai",
            "path": path,
            "auth": auth,
            "session_update": None,
            "audio_bytes": 0,
            "committed": False,
        }
        try:
            ws_send_json(sock, {"type": "session.created", "session": {"id": "sess_mock"}})
            emitted_live = 0
            with self.lock:
                live_deltas = list(self.openai_ws_live_deltas)
                final_suffix = self.openai_ws_final_suffix
            while True:
                frame = ws_recv_frame(sock)
                if frame is None:
                    break
                opcode, payload = frame
                if opcode == 0x8:
                    break
                if opcode == 0x9:
                    sock.sendall(bytes([0x8A, len(payload)]) + payload)
                    continue
                if opcode != 0x1:
                    continue
                ev = json.loads(payload.decode("utf-8"))
                etype = ev.get("type")
                if etype == "session.update":
                    record["session_update"] = ev.get("session")
                    ws_send_json(sock, {"type": "session.updated", "session": ev.get("session")})
                elif etype == "input_audio_buffer.append":
                    raw_pcm = base64.b64decode(ev.get("audio", ""))
                    record["audio_bytes"] += len(raw_pcm)
                    # Emit live deltas WHILE user is still holding the mic button!
                    while emitted_live < len(live_deltas) and record["audio_bytes"] >= (emitted_live + 1) * 2400:
                        ws_send_json(sock, {
                            "type": "conversation.item.input_audio_transcription.delta",
                            "delta": live_deltas[emitted_live],
                        })
                        emitted_live += 1
                elif etype == "input_audio_buffer.commit":
                    record["committed"] = True
                    while emitted_live < len(live_deltas):
                        ws_send_json(sock, {
                            "type": "conversation.item.input_audio_transcription.delta",
                            "delta": live_deltas[emitted_live],
                        })
                        emitted_live += 1
                    if final_suffix:
                        ws_send_json(sock, {
                            "type": "conversation.item.input_audio_transcription.delta",
                            "delta": final_suffix,
                        })
                    full_txt = "".join(live_deltas) + final_suffix
                    ws_send_json(sock, {
                        "type": "conversation.item.input_audio_transcription.completed",
                        "transcript": full_txt,
                    })
                    break
        except OSError:
            pass
        finally:
            with self.lock:
                self.ws_sessions.append(record)

    def _serve_google_ws(self, sock, path: str) -> None:
        record = {
            "provider": "google",
            "path": path,
            "setup": None,
            "context_text": "",
            "audio_bytes": 0,
            "stream_ended": False,
        }
        try:
            sent_interim = False
            sent_segment = False
            with self.lock:
                interim_txt = self.google_ws_interim
                segment_txt = self.google_ws_segment
            while True:
                frame = ws_recv_frame(sock)
                if frame is None:
                    break
                opcode, payload = frame
                if opcode == 0x8:
                    break
                if opcode == 0x9:
                    sock.sendall(bytes([0x8A, len(payload)]) + payload)
                    continue
                if opcode != 0x1:
                    continue
                ev = json.loads(payload.decode("utf-8"))
                if "setup" in ev:
                    record["setup"] = ev["setup"]
                    ws_send_json(sock, {"setupComplete": {}})
                elif "clientContent" in ev:
                    turns = ev["clientContent"].get("turns") or []
                    if turns and turns[0].get("parts"):
                        record["context_text"] = turns[0]["parts"][0].get("text", "")
                elif "realtimeInput" in ev:
                    ri = ev["realtimeInput"]
                    if "audio" in ri:
                        raw_pcm = base64.b64decode(ri["audio"].get("data", ""))
                        record["audio_bytes"] += len(raw_pcm)
                        if not sent_interim and record["audio_bytes"] >= 1600:
                            sent_interim = True
                            ws_send_json(sock, {
                                "serverContent": {
                                    "interimInputTranscription": {"text": interim_txt}
                                }
                            })
                        elif sent_interim and not sent_segment and record["audio_bytes"] >= 6400:
                            sent_segment = True
                            ws_send_json(sock, {
                                "serverContent": {
                                    "inputTranscription": {"text": segment_txt}
                                }
                            })
                    if ri.get("audioStreamEnd") is True:
                        record["stream_ended"] = True
                        if not sent_segment:
                            sent_segment = True
                            ws_send_json(sock, {
                                "serverContent": {
                                    "inputTranscription": {"text": segment_txt}
                                }
                            })
                        ws_send_json(sock, {
                            "serverContent": {"generationComplete": True, "turnComplete": True}
                        })
                        break
        except OSError:
            pass
        finally:
            with self.lock:
                self.ws_sessions.append(record)

    def write_voice_json(self, path: Path) -> Path:
        doc = {
            "openai": {
                "api_key": "sk-test-openai-voice-key",
                "transcribe_url": self.openai_transcribe_url,
                "realtime_url": self.openai_realtime_url,
            },
            "google": {
                "api_key": "AQ.test-google-voice-key",
                "api_base": self.google_api_base,
                "realtime_url": self.google_realtime_url,
            },
            "openrouter": {
                "api_key": "sk-or-test-voice-key",
                "api_url": self.openrouter_url,
            },
        }
        path.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
        os.chmod(path, 0o600)
        return path

    def clear(self) -> None:
        with self.lock:
            self.http_requests.clear()
            self.ws_sessions.clear()
        self.slow_started.clear()
        self.slow_release.clear()

    def http_snapshot(self) -> list[dict]:
        with self.lock:
            return list(self.http_requests)

    def ws_snapshot(self) -> list[dict]:
        with self.lock:
            return list(self.ws_sessions)

    def close(self) -> None:
        self.slow_release.set()
        self.server.shutdown()
        self.server.server_close()


def select_provider_and_model(page, mic, provider_id: str, model_id: str) -> None:
    mic.click()
    menu = page.locator(".voice-model-menu:popover-open")
    expect(menu).to_be_visible()
    tab = menu.locator(f'.voice-provider-tab[data-provider-id="{provider_id}"]')
    expect(tab).to_be_visible()
    tab.click()
    expect(tab).to_have_attribute("aria-selected", "true")
    opt = menu.locator(f'.voice-model-option[data-model-id="{model_id}"]')
    expect(opt).to_be_visible()
    expect(opt).to_be_enabled()
    opt.click()
    expect(page.locator(".voice-model-menu:popover-open")).to_have_count(0)
    expect(mic).to_have_attribute("data-voice-model", model_id)


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


def extract_or_prompt_text(req: dict) -> str:
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


def extract_or_audio_b64(req: dict) -> str:
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


def assert_context_clean(prompt_text: str) -> None:
    assert "请检查 SessionDock 里 codex-sdk-compose.js 的草稿同步" in prompt_text, prompt_text
    assert "我已核对 MyTavern 与 SessionDock 的 VoiceInput 热词提取接口" in prompt_text, prompt_text
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
        extra_env={"SESSIONDOCK_VOICE_CONFIG": "off"},
    ) as (base, opener):
        meta = json.loads(opener.open(f"{base}/api/meta", timeout=5).read())
        assert meta["capabilities"]["voice_input"] is False, meta["capabilities"]
        assert meta["capabilities"]["voice_providers"] == [], meta["capabilities"]
        assert meta["capabilities"]["conversation_send"] is True, meta["capabilities"]

        for method, path, payload in [
            ("POST", "/api/voice/transcribe", json.dumps({"audio": "UklGRg==", "format": "wav"}).encode("utf-8")),
            ("GET", "/api/voice/realtime", None),
        ]:
            req = urllib.request.Request(
                f"{base}{path}",
                data=payload,
                headers={"Content-Type": "application/json", "Origin": base} if payload else {"Origin": base},
                method=method,
            )
            try:
                opener.open(req, timeout=5)
                raise AssertionError(f"Expected HTTP 501 for {path} when voice_input is disabled")
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
    print("PASS capability gate (SESSIONDOCK_VOICE_CONFIG=off): hidden mic buttons + HTTP 501 on HTTP & WS", flush=True)


def test_three_providers_realtime_and_http(
    pw,
    binary: Path,
    root: Path,
    upstream: MockVoiceUpstream,
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
    voice_json = upstream.write_voice_json(root / "voice.json")

    with private_hosts(corpus.root), isolated_server(
        corpus,
        binary,
        **dirs,
        extra_env={"SESSIONDOCK_VOICE_CONFIG": str(voice_json)},
    ) as (base, opener):
        meta = json.loads(opener.open(f"{base}/api/meta", timeout=5).read())
        assert meta["capabilities"]["voice_input"] is True
        assert meta["capabilities"]["voice_providers"] == ["openai", "google", "openrouter"], meta["capabilities"]

        browser = launch_audio_browser(pw, tone_wav)
        try:
            context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
            page = context.new_page()
            errors: list[str] = []
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(base, wait_until="networkidle")

            keys = page.evaluate("() => Object.keys(localStorage)")
            assert all(k.startswith("sessiondock.") for k in keys), keys
            assert "sessiondock.voiceModel" not in keys, keys

            page.locator(".item").first.click()
            cinput = page.locator("#cinput")
            expect(cinput).to_be_visible()
            mic = page.locator("#composer [data-voice-input]")
            expect(mic).to_be_visible()
            # Default model when OpenAI is configured is openai:gpt-live-transcribe (realtime=true)
            expect(mic).to_have_attribute("data-voice-model", "openai:gpt-live-transcribe")
            expect(mic).to_have_attribute("data-voice-realtime", "true")

            # Expand folded turn-process to prove even expanded process/tool DOM is excluded!
            if page.locator("#msgs .turn-process .fold-toggle").count():
                page.locator("#msgs .turn-process .fold-toggle").first.click()
                expect(page.locator("#msgs .turn-process-body")).to_be_visible()

            # 1) Verify 3-provider tabbed menu: NO header title, upward placement, tabs at bottom & steady Y!
            mic.click()
            menu = page.locator(".voice-model-menu:popover-open")
            expect(menu).to_be_visible()
            expect(menu).to_have_attribute("data-placement", "above")
            expect(menu.locator(".voice-model-menu-head")).to_have_count(0)
            tabs_bar = menu.locator(".voice-provider-tabs")
            model_list = menu.locator(".voice-model-list")
            tabs = menu.locator(".voice-provider-tab")
            expect(tabs).to_have_count(3)
            assert tabs.evaluate_all("els => els.map(e => e.dataset.providerId)") == ["openai", "google", "openrouter"]
            assert menu.locator(".voice-model-option").evaluate_all("els => els.map(e => e.dataset.modelId)") == EXPECTED_OPENAI_MODELS
            expect(menu.locator('.voice-model-option[data-model-id="openai:gpt-live-transcribe"] .voice-realtime-badge')).to_have_text("实时")

            mic_box = mic.bounding_box()
            menu_box_oa = menu.bounding_box()
            list_box_oa = model_list.bounding_box()
            tabs_box_oa = tabs_bar.bounding_box()
            assert mic_box and menu_box_oa and list_box_oa and tabs_box_oa
            # Popover opens upward above mic, model list is on top, provider tabs are at the bottom nearest mic!
            assert menu_box_oa["y"] + menu_box_oa["height"] <= mic_box["y"] + 1, (menu_box_oa, mic_box)
            assert tabs_box_oa["y"] >= list_box_oa["y"] + list_box_oa["height"] - 1, (list_box_oa, tabs_box_oa)

            if output:
                page.screenshot(path=str(output / "node-1280-light-openai-tab.png"), full_page=True)

            # Switch to Google tab -> verify 3 Google models + `实时` badge on gemini-3.5-transcribe-live
            tabs.nth(1).click()
            assert menu.locator(".voice-model-option").evaluate_all("els => els.map(e => e.dataset.modelId)") == EXPECTED_GOOGLE_MODELS
            expect(menu.locator('.voice-model-option[data-model-id="google:gemini-3.5-transcribe-live"] .voice-realtime-badge')).to_have_text("实时")
            if output:
                page.screenshot(path=str(output / "node-1280-light-google-tab.png"), full_page=True)

            # Switch to OpenRouter tab (4 models) -> menu grows upward while bottom tabs stay at the exact same Y!
            tabs.nth(2).click()
            assert menu.locator(".voice-model-option").evaluate_all("els => els.map(e => e.dataset.modelId)") == EXPECTED_OPENROUTER_MODELS
            tabs_box_or = menu.locator(".voice-provider-tabs").bounding_box()
            assert tabs_box_or and abs(tabs_box_or["y"] - tabs_box_oa["y"]) <= 1.5, (tabs_box_oa, tabs_box_or)
            if output:
                page.screenshot(path=str(output / "node-1280-light-openrouter-tab.png"), full_page=True)

            # Close menu and test default OpenAI Realtime (`openai:gpt-live-transcribe`) LIVE streaming while holding!
            mic.click()
            expect(page.locator(".voice-model-menu:popover-open")).to_have_count(0)

            # 2) OpenAI Realtime (`openai:gpt-live-transcribe`): words appear in #cinput BEFORE mouse.up()!
            upstream.clear()
            cinput.fill("开头： [结尾]")
            cinput.focus()
            cinput.evaluate("el => el.setSelectionRange(3, 3)")

            box = mic.bounding_box()
            assert box
            page.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
            page.mouse.down()
            expect(mic).to_have_attribute("data-voice-state", "recording", timeout=4000)
            live_card = page.locator(".voice-status-pill:popover-open .voice-live-card")
            expect(live_card).to_be_visible(timeout=4000)
            expect(live_card.locator(".voice-live-badge")).to_have_text("实时听写")

            # Assert live words arrive in <textarea> AND in `.voice-live-card` WHILE STILL HOLDING!
            expect(cinput).to_have_value("开头：边说话边出字 [结尾]", timeout=6000)
            expect(live_card.locator(".voice-live-committed")).to_have_text("边说话边出字")
            expect(mic).to_have_attribute("data-voice-state", "recording")

            if output:
                page.screenshot(path=str(output / "node-1280-light-realtime-live-card.png"), full_page=True)

            # Now release mouse -> commits buffer -> receives final suffix -> closes card!
            page.mouse.up()
            expect(cinput).to_have_value("开头：边说话边出字，实时完成！ [结尾]", timeout=6000)
            expect(mic).to_have_attribute("data-voice-state", "idle", timeout=5000)

            ws_reqs = upstream.ws_snapshot()
            assert len(ws_reqs) == 1, ws_reqs
            assert ws_reqs[0]["provider"] == "openai"
            assert ws_reqs[0]["auth"] == "Bearer sk-test-openai-voice-key"
            assert ws_reqs[0]["committed"] is True
            oa_prompt = ws_reqs[0]["session_update"]["audio"]["input"]["transcription"]["prompt"]
            assert_context_clean(oa_prompt)
            assert "开头：" in oa_prompt and "[结尾]" in oa_prompt, oa_prompt

            # 3) Real-time cancel (`Escape` AND slide-up) rolls back live-inserted words cleanly!
            upstream.clear()
            cinput.fill("保留原稿")
            cinput.focus()
            cinput.evaluate("el => el.setSelectionRange(4, 4)")
            page.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
            page.mouse.down()
            expect(cinput).to_have_value("保留原稿边说话边出字", timeout=6000)
            page.keyboard.press("Escape")
            page.mouse.up()
            expect(mic).to_have_attribute("data-voice-state", "idle", timeout=4000)
            expect(cinput).to_have_value("保留原稿")

            # Slide-up rollback after live words arrived
            page.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
            page.mouse.down()
            expect(cinput).to_have_value("保留原稿边说话边出字", timeout=6000)
            page.mouse.move(box["x"] + box["width"] / 2, box["y"] - 80, steps=6)
            expect(page.locator(".voice-live-card .voice-live-badge")).to_have_text("松手撤回")
            page.mouse.up()
            expect(mic).to_have_attribute("data-voice-state", "idle", timeout=4000)
            expect(cinput).to_have_value("保留原稿")

            # 4) Google AI Studio Realtime (`google:gemini-3.5-transcribe-live`): interim preview + segment!
            select_provider_and_model(page, mic, "google", "google:gemini-3.5-transcribe-live")
            expect(mic).to_have_attribute("data-voice-realtime", "true")
            upstream.clear()
            cinput.fill("")
            box = mic.bounding_box()
            assert box
            page.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
            page.mouse.down()
            expect(cinput).to_have_value("Gemini Live 实时流式输出", timeout=6000)
            page.mouse.up()
            expect(mic).to_have_attribute("data-voice-state", "idle", timeout=5000)
            expect(cinput).to_have_value("Gemini Live 实时流式输出")
            g_ws = upstream.ws_snapshot()
            assert len(g_ws) == 1 and g_ws[0]["provider"] == "google", g_ws
            assert g_ws[0]["setup"]["model"] == "models/gemini-3.5-transcribe-live"
            assert g_ws[0]["stream_ended"] is True
            assert_context_clean(g_ws[0]["context_text"])

            # 5) OpenAI HTTP/SSE (`openai:gpt-4o-transcribe`)
            select_provider_and_model(page, mic, "openai", "openai:gpt-4o-transcribe")
            expect(mic).to_have_attribute("data-voice-realtime", "false")
            upstream.clear()
            cinput.fill("")
            long_press_mic(page, mic, hold_ms=360)
            expect(cinput).to_have_value("OpenAI GPT-4o 流式转写完成", timeout=8000)
            expect(mic).to_have_attribute("data-voice-state", "idle", timeout=5000)
            h_reqs = upstream.http_snapshot()
            assert len(h_reqs) == 1 and h_reqs[0]["provider"] == "openai", h_reqs
            assert h_reqs[0]["auth"] == "Bearer sk-test-openai-voice-key"
            assert h_reqs[0]["model"] == "gpt-4o-transcribe"
            assert h_reqs[0]["stream"] == "true"
            assert_valid_16k_mono_wav_bytes(h_reqs[0]["file"])
            assert_context_clean(h_reqs[0]["prompt"])

            # 6) Google AI Studio HTTP/SSE (`google:gemini-3.8-flash`)
            select_provider_and_model(page, mic, "google", "google:gemini-3.8-flash")
            upstream.clear()
            cinput.fill("")
            long_press_mic(page, mic, hold_ms=360)
            expect(cinput).to_have_value("Google Gemini 3.8 Flash 转写成功", timeout=8000)
            expect(mic).to_have_attribute("data-voice-state", "idle", timeout=5000)
            h_reqs = upstream.http_snapshot()
            assert len(h_reqs) == 1 and h_reqs[0]["provider"] == "google", h_reqs
            assert h_reqs[0]["api_key"] == "AQ.test-google-voice-key"
            assert "/v1beta/models/gemini-3.8-flash:streamGenerateContent?alt=sse" in h_reqs[0]["path"]
            g_parts = h_reqs[0]["payload"]["contents"][0]["parts"]
            assert_context_clean(g_parts[0]["text"])
            assert_valid_16k_mono_wav(g_parts[1]["inlineData"]["data"])

            # 7) OpenRouter HTTP/SSE (`openrouter:qwen/qwen3.8-omni-flash`), slow abort & refusal filter
            select_provider_and_model(page, mic, "openrouter", "openrouter:qwen/qwen3.8-omni-flash")
            upstream.clear()
            cinput.fill("")
            long_press_mic(page, mic, hold_ms=360)
            expect(cinput).to_have_value("OpenRouter 语音输入完成", timeout=8000)
            h_reqs = upstream.http_snapshot()
            assert len(h_reqs) == 1 and h_reqs[0]["provider"] == "openrouter", h_reqs
            assert h_reqs[0]["payload"]["model"] == "qwen/qwen3.8-omni-flash"
            assert_valid_16k_mono_wav(extract_or_audio_b64(h_reqs[0]))
            assert_context_clean(extract_or_prompt_text(h_reqs[0]))

            # Slow SSE stream abort on click
            upstream.clear()
            upstream.or_mode = "slow"
            cinput.fill("")
            box = mic.bounding_box()
            assert box
            page.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
            page.mouse.down()
            expect(mic).to_have_attribute("data-voice-state", "recording", timeout=4000)
            page.wait_for_timeout(340)
            page.mouse.up()
            assert upstream.slow_started.wait(timeout=5.0)
            expect(mic).to_have_attribute("data-voice-state", "transcribing", timeout=4000)
            mic.click()
            upstream.slow_release.set()
            expect(mic).to_have_attribute("data-voice-state", "idle", timeout=4000)
            assert "不应完整到达" not in cinput.input_value()
            upstream.or_mode = "normal"

            # Refusal filter suppresses chatty model refusal
            upstream.clear()
            upstream.or_mode = "refusal"
            cinput.fill("保留原稿")
            long_press_mic(page, mic, hold_ms=340)
            expect(mic).to_have_attribute("data-voice-state", "idle", timeout=5000)
            assert cinput.input_value() == "保留原稿"
            upstream.or_mode = "normal"

            assert not errors, errors
            context.close()
        finally:
            browser.close()

        # Silence guard with pure-zero WAV capture (both realtime and batch!)
        silent_browser = launch_audio_browser(pw, silent_wav)
        try:
            context = silent_browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
            page = context.new_page()
            page.goto(base, wait_until="networkidle")
            page.locator(".item").first.click()
            mic = page.locator("#composer [data-voice-input]")
            expect(mic).to_be_visible()
            select_provider_and_model(page, mic, "openai", "openai:gpt-4o-mini-transcribe")
            upstream.clear()
            long_press_mic(page, mic, hold_ms=360)
            expect(mic).to_have_attribute("data-voice-state", "idle")
            expect(page.locator(".voice-status-pill:popover-open")).to_contain_text("未检测到语音")
            assert len(upstream.http_snapshot()) == 0
            context.close()
        finally:
            silent_browser.close()

    print("PASS 3 providers (OpenAI, Google, OpenRouter), live WebSocket dictation + rollback, HTTP SSE, context & silence", flush=True)


def exercise_sdk_dialogs_and_hub(
    browser,
    url: str,
    label: str,
    width: int,
    upstream: MockVoiceUpstream,
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

    page.locator(".item").first.click()
    sdk_input = page.locator("#sdk-input")
    expect(sdk_input).to_be_visible(timeout=10000)
    sdk_mic = page.locator("#sdk-composer [data-voice-input]")
    expect(sdk_mic).to_be_visible()
    expect(sdk_input).to_be_enabled(timeout=10000)
    expect(sdk_mic).to_be_enabled(timeout=10000)

    # On node: test OpenAI realtime (`openai:gpt-live-transcribe`).
    # On hub: test Google realtime (`google:gemini-3.5-transcribe-live`) over Hub -> Node zero-copy WS proxy!
    chosen_provider = "openai" if label == "node" else "google"
    chosen_model = "openai:gpt-live-transcribe" if label == "node" else "google:gemini-3.5-transcribe-live"

    sdk_mic.click()
    menu = page.locator(".voice-model-menu:popover-open")
    expect(menu).to_be_visible()
    menu.locator(f'.voice-provider-tab[data-provider-id="{chosen_provider}"]').click()
    if output:
        page.screenshot(path=str(output / f"{label}-{width}-{'dark' if dark else 'light'}-sdk-menu.png"), full_page=True)
    menu.locator(f'.voice-model-option[data-model-id="{chosen_model}"]').click()
    expect(sdk_mic).to_have_attribute("data-voice-model", chosen_model)
    expect(sdk_mic).to_have_attribute("data-voice-realtime", "true")

    # Live dictate into #sdk-input while holding!
    sdk_input.fill("")
    upstream.clear()
    box = sdk_mic.bounding_box()
    assert box
    page.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
    page.mouse.down()
    expect(sdk_mic).to_have_attribute("data-voice-state", "recording", timeout=4000)
    expected_live = "边说话边出字" if label == "node" else "Gemini Live 实时流式输出"
    expect(sdk_input).to_have_value(expected_live, timeout=6000)
    if output:
        page.screenshot(path=str(output / f"{label}-{width}-{'dark' if dark else 'light'}-sdk-live-dictation.png"), full_page=True)
    page.mouse.up()
    expected_final = "边说话边出字，实时完成！" if label == "node" else "Gemini Live 实时流式输出"
    expect(sdk_input).to_have_value(expected_final, timeout=6000)
    expect(sdk_mic).to_have_attribute("data-voice-state", "idle", timeout=5000)

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
    page.locator("#new-session-dialog .modal-close").click()
    expect(page.locator("#new-session-dialog")).to_be_hidden()
    expect(page.locator(".voice-model-menu:popover-open")).to_have_count(0)

    if width < 720 and page.locator(".mobile-back").is_visible():
        page.locator(".mobile-back").click()

    # Open Bug Report dialog (#bug-report-description inside #bug-report-dialog) and test HTTP SSE on Hub/Node too!
    open_report(page)
    bug_input = page.locator("#bug-report-description")
    bug_mic = page.locator("#bug-report-form [data-voice-input]")
    expect(bug_input).to_be_visible()
    expect(bug_mic).to_be_visible()
    select_provider_and_model(page, bug_mic, "openai", "openai:gpt-4o-mini-transcribe")
    expect(page.locator("#bug-report-dialog")).to_be_visible()

    upstream.clear()
    long_press_mic(page, bug_mic, hold_ms=360)
    expect(bug_input).to_have_value("OpenAI GPT-4o 流式转写完成", timeout=8000)
    page.locator("#bug-report-dialog .modal-close").click()

    assert not errors, errors
    context.close()
    print(f"PASS {label} {width}px ({'dark' if dark else 'light'}): SDK live WS dictation + Continue + BugReport HTTP SSE", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--output", type=Path, default=None)
    args = parser.parse_args()
    if args.output:
        args.output.mkdir(parents=True, exist_ok=True)

    upstream = MockVoiceUpstream()
    try:
        with tempfile.TemporaryDirectory(prefix="sd-voice-input-") as tmp, sync_playwright() as pw:
            root = Path(tmp)
            tone_wav = root / "tone.wav"
            silent_wav = root / "silent.wav"
            write_wav_fixture(tone_wav, silent=False)
            write_wav_fixture(silent_wav, silent=True)

            test_capability_gate_disabled(pw, args.binary, root, tone_wav)
            test_three_providers_realtime_and_http(pw, args.binary, root, upstream, tone_wav, silent_wav, args.output)

            sdk_root = root / "sdk-hub"
            sdk_root.mkdir()
            voice_json = upstream.write_voice_json(sdk_root / "voice.json")
            with provider(
                "codex",
                sdk_root,
                args.binary,
                report=True,
                extra_env={"SESSIONDOCK_VOICE_CONFIG": str(voice_json)},
            ) as (node_url, _, extra):
                hub_root = sdk_root / "hub"
                hub_root.mkdir()
                node_port = int(extra["SESSIONDOCK_NODE_BIND"].rsplit(":", 1)[1])
                node_desc = SimpleNamespace(name="VoiceNode", token=TOKEN, port=node_port)
                hub = Hub(args.binary.resolve().with_name("sessiondock-hub"), hub_root, [node_desc])
                # Force Hub itself to have NO local voice config so it proxies BOTH WS /api/voice/realtime
                # and POST /api/voice/transcribe to VoiceNode!
                hub.env["SESSIONDOCK_VOICE_CONFIG"] = "off"
                hub.env.pop("OPENROUTER_API_KEY", None)
                hub.env.pop("OPENAI_API_KEY", None)
                hub.env.pop("GEMINI_API_KEY", None)
                hub.env.pop("GOOGLE_API_KEY", None)
                hub.start()

                browser = launch_audio_browser(pw, tone_wav)
                try:
                    for label, url in [("node", node_url), ("hub", f"http://127.0.0.1:{hub.port}")]:
                        for width in (1280, 390):
                            exercise_sdk_dialogs_and_hub(browser, url, label, width, upstream, args.output)
                finally:
                    browser.close()
                    hub.stop()
    finally:
        upstream.close()


if __name__ == "__main__":
    main()
