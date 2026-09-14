#!/usr/bin/env python3
# run_validation: skip
"""Fake Claude CLI for reliable-send tests. Never a model binary.

Launched by the lifecycle launcher exactly like a real profile
(`--session-id <uuid>` for new sessions, `--resume <sid>` for resumes). It
renders a Claude-like composer on the PTY (two rules and a `❯` row), runs a
tiny raw-mode line editor (printable input, bracketed paste, C-u/C-k clear,
Backspace, Enter), and for every submitted line appends a synthetic Claude
`user` JSONL record (uuid, parentUuid chain, sessionId, message.content = the
line) to `$SESSIONDOCK_TEST_CLAUDE_ROOT/project-history/<sid>.jsonl`. Options:

  --delay MS       write the record MS milliseconds after Enter (busy TUI)
  --swallow N      drop the N-th submitted line (1-based) without any record
  --reply          also append a synthetic assistant reply after each user record
  --busy-footer    show Claude's "esc to interrupt" footer while delayed
"""
import json
import os
import sys
import termios
import time
import tty
import uuid

RULE = "─" * 48


def parse(argv):
    options = {"sid": "", "delay": 0.0, "swallow": 0, "reply": False,
               "busy_footer": False}
    i = 0
    while i < len(argv):
        arg = argv[i]
        if arg in ("--session-id", "--resume") and i + 1 < len(argv):
            options["sid"] = argv[i + 1]
            i += 1
        elif arg == "--delay" and i + 1 < len(argv):
            options["delay"] = float(argv[i + 1]) / 1000.0
            i += 1
        elif arg == "--swallow" and i + 1 < len(argv):
            options["swallow"] = int(argv[i + 1])
            i += 1
        elif arg == "--reply":
            options["reply"] = True
        elif arg == "--busy-footer":
            options["busy_footer"] = True
        elif arg in ("--settings", "--model", "--effort") and i + 1 < len(argv):
            i += 1
        i += 1
    return options


class Fake:
    def __init__(self, options):
        self.sid = options["sid"]
        self.options = options
        self.buffer = ""
        self.transcript = []
        self.parent = None
        self.submitted = 0
        root = os.environ.get("SESSIONDOCK_TEST_CLAUDE_ROOT", "")
        self.path = os.path.join(root, "project-history", f"{self.sid}.jsonl") if root and self.sid else ""
        # A resumed session chains from the file's last record like the real
        # CLI does; a second root would be a separate branch the read model
        # never shows as the active timeline's new input.
        if self.path and os.path.isfile(self.path):
            for line in reversed(open(self.path, encoding="utf-8").read().splitlines()):
                try:
                    record = json.loads(line)
                except ValueError:
                    continue
                if isinstance(record, dict) and record.get("uuid"):
                    self.parent = record["uuid"]
                    break
        self.out = sys.stdout

    def write(self, text):
        self.out.write(text)
        self.out.flush()

    def render(self, footer=""):
        lines = ["FAKE_CLAUDE_READY sid=[%s]" % self.sid]
        for text in self.transcript[-4:]:
            lines.extend(("> " + text).splitlines())
        lines.append("")
        lines.append(RULE)
        # Let the terminal compute the cursor position: multiline attachments,
        # wrapping and wide Unicode glyphs make string lengths/row counts wrong.
        # Preserve the actual composer cursor while drawing its bottom rule.
        body = "\r\n".join(lines) + "\r\n❯ " + self.buffer.replace("\n", "\r\n")
        self.write("\x1b[2J\x1b[H" + body + "\x1b7\r\n" + RULE
                   + ("\r\n" + footer if footer else "") + "\x1b8")

    def record(self, text):
        if not self.path:
            return
        os.makedirs(os.path.dirname(self.path), exist_ok=True)
        now = time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime()) + ".%03dZ" % int((time.time() % 1) * 1000)
        user_uuid = str(uuid.uuid4())
        rows = [{
            "type": "user", "uuid": user_uuid, "parentUuid": self.parent,
            "sessionId": self.sid, "cwd": os.getcwd(), "timestamp": now,
            "isSidechain": False, "userType": "external", "version": "fake-2.1",
            "message": {"role": "user", "content": text},
        }]
        self.parent = user_uuid
        if self.options["reply"]:
            reply_uuid = str(uuid.uuid4())
            rows.append({
                "type": "assistant", "uuid": reply_uuid, "parentUuid": user_uuid,
                "sessionId": self.sid, "cwd": os.getcwd(), "timestamp": now,
                "isSidechain": False, "userType": "external", "version": "fake-2.1",
                "message": {"role": "assistant", "model": "fake-model", "stop_reason": "end_turn",
                            "content": [{"type": "text", "text": "OK: " + text}]},
            })
            self.parent = reply_uuid
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
            footer = "✻ Thinking… (esc to interrupt)" if self.options["busy_footer"] else ""
            self.render(footer)
            time.sleep(self.options["delay"])
        self.record(text)
        self.render()

    def run(self):
        fd = sys.stdin.fileno()
        saved = termios.tcgetattr(fd)
        tty.setraw(fd)
        try:
            self.write("\x1b[?2004h")  # bracketed paste on, like Claude
            self.render()
            pending = b""
            paste = None
            while True:
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
                        self.buffer += paste.decode("utf-8", "replace").replace("\r", "")
                        paste = None
                        self.render()
                        continue
                    if pending.startswith(b"\x1b[200~"):
                        paste = b""
                        pending = pending[6:]
                        continue
                    byte = pending[:1]
                    if byte == b"\x1b":
                        # Swallow an escape sequence (arrow keys etc.).
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
                    # Printable UTF-8 text typed or sent without bracketing:
                    # consume up to the next control byte; keep an incomplete
                    # trailing multibyte sequence for the next chunk.
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
                                break  # wait for the rest of the sequence
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
