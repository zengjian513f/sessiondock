#!/usr/bin/env python3
# run_validation: skip
"""Fake Codex CLI for reliable-send tests. Never a model binary.

Launched by the lifecycle launcher exactly like a real profile (`resume <sid>`
on a session the frozen inventory already lists; `-c key=value`, `--model` and
similar real flags are accepted and ignored). It renders a Codex-style
composer on the PTY the way Codex 0.154 does: a `›` input row showing a dim
rotating placeholder while empty, braille "particle" glyphs (U+2800–U+28FF,
painted in RGB colour, never dim) animated through the padding rows and the
blank cells of the input row, and a `model · cwd` status footer as the last
line (a `Working (… esc to interrupt)` status while a turn is running). It
runs a tiny raw-mode line editor (printable input, bracketed paste, C-u/C-k
clear, Backspace, Enter) and for every submitted line appends the real
rollout shape to the resumed rollout under `$SESSIONDOCK_TEST_CODEX_ROOT`:

  turn_context {model, effort, turn_id}
  event_msg task_started {turn_id}
  response_item message role=user {internal_chat_message_metadata_passthrough.turn_id,
                                   content:[{type:input_text,text}]}
  event_msg user_message {message}
  (optional --reply: response_item message role=assistant)
  event_msg task_complete {turn_id}

Options:
  --delay MS       write the turn MS milliseconds after Enter (slow TUI)
  --swallow N      drop the N-th submitted line (1-based) without any record
  --no-turn-id     omit the user record's turn identity (text still confirms)
  --duplicate      write the user record twice (ambiguous for the adapter)
  --reply          also append a synthetic assistant reply before task_complete
  --busy-footer    show Codex's "Working … esc to interrupt" status while delayed
"""
import json
import os
import select
import sys
import termios
import time
import tty
import uuid

PLACEHOLDER = "Ask Codex to do anything"
PARTICLES = ["⠁⠂⠄ ⠈⠐⠠", "⠐⠠⠁ ⠂⠄⠈", "⠄⠈⠐ ⠠⠁⠂"]
DIM = "\x1b[2m"
RESET = "\x1b[0m"
GREY = "\x1b[38;2;90;90;90m"


def parse(argv):
    options = {"sid": "", "delay": 0.0, "swallow": 0, "reply": False,
               "busy_footer": False, "no_turn_id": False, "duplicate": False,
               "model": "gpt-5.6-luna"}
    i = 0
    while i < len(argv):
        arg = argv[i]
        if arg == "resume" and i + 1 < len(argv):
            options["sid"] = argv[i + 1]
            i += 1
        elif arg == "--delay" and i + 1 < len(argv):
            options["delay"] = float(argv[i + 1]) / 1000.0
            i += 1
        elif arg == "--swallow" and i + 1 < len(argv):
            options["swallow"] = int(argv[i + 1])
            i += 1
        elif arg == "--no-turn-id":
            options["no_turn_id"] = True
        elif arg == "--duplicate":
            options["duplicate"] = True
        elif arg == "--reply":
            options["reply"] = True
        elif arg == "--busy-footer":
            options["busy_footer"] = True
        elif arg in ("--model", "-m") and i + 1 < len(argv):
            options["model"] = argv[i + 1]
            i += 1
        elif arg in ("-c", "--config", "--sandbox", "-s", "--profile", "-p") and i + 1 < len(argv):
            i += 1
        i += 1
    return options


def find_rollout(root, sid):
    """The rollout whose `session_meta.payload.id` is `sid`; by name first."""
    if not root or not sid:
        return ""
    fallback = ""
    for directory, _, files in os.walk(root):
        for name in sorted(files):
            if not name.endswith(".jsonl"):
                continue
            path = os.path.join(directory, name)
            if name == f"rollout-{sid}.jsonl" or (name.startswith("rollout-") and sid in name):
                return path
            if not fallback:
                try:
                    with open(path, "rb") as stream:
                        head = json.loads(stream.readline(65536))
                except (OSError, ValueError):
                    continue
                if head.get("type") == "session_meta" and (head.get("payload") or {}).get("id") == sid:
                    fallback = path
    return fallback


class Fake:
    def __init__(self, options):
        self.sid = options["sid"]
        self.options = options
        self.buffer = ""
        self.transcript = []
        self.submitted = 0
        self.turns = 0
        self.frame = 0
        self.path = find_rollout(os.environ.get("SESSIONDOCK_TEST_CODEX_ROOT", ""), self.sid)
        self.out = sys.stdout

    def write(self, text):
        self.out.write(text)
        self.out.flush()

    def particles(self):
        self.frame += 1
        return GREY + PARTICLES[self.frame % len(PARTICLES)] + RESET

    def animated_padding(self):
        # Real Codex particles replace blank cells, rather than adding cells.
        count = (self.frame // 2) % 20
        return '\x1b[48;2;30;30;30m' + GREY + '⠁' * count + ' ' * (20 - count) + RESET

    def render(self, working=False):
        lines = ["FAKE_CODEX_TUI sid=[%s]" % self.sid]
        for text in self.transcript[-4:]:
            parts = text.split("\n")
            lines.append("> " + parts[0])
            lines.extend("  " + part for part in parts[1:])
        lines.append("")
        if working:
            lines.append("• Working (1s • esc to interrupt)")
        lines.append(self.particles())
        prompt_row = len(lines) + 1
        if self.buffer:
            # A long paste must not scroll › or the model footer off the 36-row
            # PTY; inspect_codex needs both on the captured screen.
            shown = self.buffer if len(self.buffer) <= 72 else self.buffer[-72:]
            parts = shown.split("\n")
            lines.append("› " + parts[0])
            lines.extend("  " + part for part in parts[1:])
            if os.environ.get('SESSIONDOCK_TEST_ANIMATED_PADDING'):
                for index in range(prompt_row - 1, len(lines)):
                    lines[index] += self.animated_padding()
        else:
            parts = [""]
            lines.append("› " + DIM + PLACEHOLDER + RESET + "   " + GREY + "⠁⠂" + RESET)
        lines.append(self.particles())
        lines.append("")
        lines.append("%s low · %s" % (self.options["model"], os.getcwd()))
        self.write("\x1b[2J\x1b[H" + "\r\n".join(lines))
        column = 3 + len(parts[-1])
        self.write("\x1b[%d;%dH" % (prompt_row + len(parts) - 1, column))

    def stamp(self):
        return time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime()) + ".%03dZ" % int((time.time() % 1) * 1000)

    def record(self, text):
        if not self.path:
            return
        self.turns += 1
        turn = str(uuid.uuid4())
        user = {"type": "message", "role": "user",
                "content": [{"type": "input_text", "text": text}]}
        if not self.options["no_turn_id"]:
            user["internal_chat_message_metadata_passthrough"] = {
                "turn_id": turn, "create_time": time.time(), "content_item_kinds": ["input_text"]}
        rows = [
            {"timestamp": self.stamp(), "type": "turn_context",
             "payload": {"cwd": os.getcwd(), "model": self.options["model"], "effort": "low",
                         "turn_id": turn}},
            {"timestamp": self.stamp(), "type": "event_msg",
             "payload": {"type": "task_started", "turn_id": turn}},
            {"timestamp": self.stamp(), "type": "response_item", "payload": user},
            {"timestamp": self.stamp(), "type": "event_msg",
             "payload": {"type": "user_message", "message": text}},
        ]
        if self.options["duplicate"]:
            rows.append({"timestamp": self.stamp(), "type": "response_item", "payload": dict(user)})
        if self.options["reply"]:
            rows.append({"timestamp": self.stamp(), "type": "response_item",
                         "payload": {"type": "message", "role": "assistant", "phase": "final_answer",
                                     "internal_chat_message_metadata_passthrough": {"turn_id": turn},
                                     "content": [{"type": "output_text", "text": "OK: " + text}]}})
        rows.append({"timestamp": self.stamp(), "type": "event_msg",
                     "payload": {"type": "task_complete", "turn_id": turn, "duration_ms": 5,
                                 "last_agent_message": "OK: " + text if self.options["reply"] else None}})
        with open(self.path, "a", encoding="utf-8") as stream:
            for row in rows:
                stream.write(json.dumps(row, ensure_ascii=False) + "\n")
            stream.flush()
            os.fsync(stream.fileno())

    def submit(self):
        text = self.buffer
        self.buffer = ""
        if not text.strip():
            self.render()
            return
        self.transcript.append(text)
        self.submitted += 1
        if self.options["swallow"] == self.submitted:
            self.render()
            return
        if self.options["delay"] > 0:
            self.render(working=self.options["busy_footer"])
            time.sleep(self.options["delay"])
        self.record(text.strip())
        self.render()

    def run(self):
        fd = sys.stdin.fileno()
        saved = termios.tcgetattr(fd)
        tty.setraw(fd)
        try:
            self.write("\x1b[?2004h")  # bracketed paste on, like Codex
            self.render()
            pending = b""
            paste = None
            while True:
                if os.environ.get('SESSIONDOCK_TEST_ANIMATED_PADDING') and not select.select([fd], [], [], .03)[0]:
                    self.render()
                    continue
                chunk = os.read(fd, 4096)
                if not chunk:
                    return 0
                pending += chunk
                while pending:
                    if paste is not None:
                        end = pending.find(b"\x1b[201~")
                        if end < 0:
                            paste += pending
                            pending = b""
                            break
                        paste += pending[:end]
                        pending = pending[end + 6:]
                        self.buffer += paste.decode("utf-8", "replace").replace("\r", "\n")
                        paste = None
                        self.render()
                        continue
                    if pending.startswith(b"\x1b[200~"):
                        paste = b""
                        pending = pending[6:]
                        continue
                    byte = pending[:1]
                    if byte == b"\x1b":
                        cut = 1
                        if len(pending) > 1 and pending[1:2] == b"[":
                            cut = 2
                            while cut < len(pending) and not (0x40 <= pending[cut] <= 0x7e):
                                cut += 1
                            cut += 1
                        pending = pending[cut:]
                        continue
                    if byte in (b"\r", b"\n"):
                        pending = pending[1:]
                        self.submit()
                        continue
                    if byte == b"\x15":  # C-u
                        pending = pending[1:]
                        self.buffer = ""
                        self.render()
                        continue
                    if byte == b"\x0b":  # C-k
                        pending = pending[1:]
                        self.render()
                        continue
                    if byte == b"\x7f":
                        pending = pending[1:]
                        self.buffer = self.buffer[:-1]
                        self.render()
                        continue
                    if byte in (b"\x03", b"\x04"):
                        return 0
                    cut = next((i for i, b in enumerate(pending) if b < 0x20 or b == 0x7f), len(pending))
                    if cut == 0:
                        pending = pending[1:]
                        continue
                    try:
                        text = pending[:cut].decode("utf-8")
                    except UnicodeDecodeError as error:
                        if error.start == 0:
                            pending = pending[1:]
                            continue
                        if error.end >= cut and cut == len(pending):
                            text = pending[:error.start].decode("utf-8")
                            cut = error.start
                            if not text:
                                break
                        else:
                            text = pending[:cut].decode("utf-8", "replace")
                    pending = pending[cut:]
                    self.buffer += text
                    self.render()
        finally:
            termios.tcsetattr(fd, termios.TCSADRAIN, saved)
            self.write("\x1b[?2004l")


if __name__ == "__main__":
    sys.exit(Fake(parse(sys.argv[1:])).run())
