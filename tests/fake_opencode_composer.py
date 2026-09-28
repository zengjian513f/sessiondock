#!/usr/bin/env python3
# run_validation: skip
"""Private, free OpenCode 2-shaped CLI for SessionDock tests.

`fake_opencode_composer.py api session.create|session.get|session.remove …`
works on an OpenCode 2 style SQLite store (`SESSIONDOCK_TEST_OPENCODE_DB`:
`project`, `session_v2`, `session_message`). Without `api` it is the TUI:
the prompt block copies OpenCode 2.0 (`┃` left border, `╹▀…` rule,
`agent · model` row); `SESSIONDOCK_TEST_SCREEN` switches it between
`composer` (default) and `palette` (the command palette moves the cursor
away). A bracketed paste shows `[Pasted ~N lines]`. With `--session <id>`
Enter stores the draft as a user message and a settled `echo: …` assistant
reply, like OpenCode would. Every input byte is appended to
`<screen>.trace`.
"""
import json
import os
from pathlib import Path
import select
import sqlite3
import sys
import termios
import time
import tty

SCHEMA = """
CREATE TABLE IF NOT EXISTS project (id text PRIMARY KEY, worktree text NOT NULL,
  time_created integer NOT NULL, time_updated integer NOT NULL, sandboxes text NOT NULL);
CREATE TABLE IF NOT EXISTS session_v2 (id text PRIMARY KEY, project_id text NOT NULL,
  parent_id text, slug text NOT NULL, directory text NOT NULL, title text, version text NOT NULL,
  agent text, model text, time_created integer NOT NULL, time_updated integer NOT NULL);
CREATE TABLE IF NOT EXISTS session_message (id text PRIMARY KEY, session_id text NOT NULL,
  type text NOT NULL, seq integer NOT NULL, time_created integer NOT NULL,
  time_updated integer NOT NULL, data text NOT NULL);
"""
MODEL = {"id": "fake-model", "providerID": "fake", "variant": "low"}


def now():
    return int(time.time() * 1000)


def database():
    connection = sqlite3.connect(os.environ["SESSIONDOCK_TEST_OPENCODE_DB"], timeout=5)
    connection.executescript(SCHEMA)
    return connection


def api(args):
    operation = args[0]
    options = dict(zip(args[1::2], args[2::2]))
    with database() as db:
        if operation == "session.create":
            body = json.loads(options["--data"])
            sid, directory = body["id"], body["location"]["directory"]
            db.execute("INSERT OR IGNORE INTO project VALUES ('fakeproject', ?, ?, ?, '[]')",
                       (directory, now(), now()))
            db.execute("INSERT INTO session_v2 (id, project_id, slug, directory, version, agent, model,"
                       " time_created, time_updated) VALUES (?, 'fakeproject', 'fake-slug', ?, '2.0.18',"
                       " 'build', ?, ?, ?)", (sid, directory, json.dumps(MODEL), now(), now()))
            print(json.dumps({"data": {"id": sid}}))
            return 0
        sid = options.get("--param", "=").split("=", 1)[1]
        if operation == "session.get":
            return 0 if db.execute("SELECT 1 FROM session_v2 WHERE id = ?", (sid,)).fetchone() else 1
        if operation == "session.remove":
            db.execute("DELETE FROM session_message WHERE session_id = ?", (sid,))
            db.execute("DELETE FROM session_v2 WHERE id = ?", (sid,))
            return 0
    return 2


def submit(session, draft):
    with database() as db:
        seq = db.execute("SELECT coalesce(max(seq), 0) FROM session_message WHERE session_id = ?",
                         (session,)).fetchone()[0]
        stamp = now()
        rows = [("user", {"time": {"created": stamp}, "text": draft, "files": [], "agents": []}),
                ("assistant", {"time": {"created": stamp, "completed": stamp + 1}, "agent": "build",
                               "model": MODEL, "finish": "stop",
                               "content": [{"type": "text", "text": "echo: " + draft}]}),
                ("idle", {"time": {"created": stamp + 2}, "outcome": "succeeded"})]
        for index, (kind, data) in enumerate(rows, 1):
            db.execute("INSERT INTO session_message VALUES (?, ?, ?, ?, ?, ?, ?)",
                       (f"msg_{session}_{seq + index}", session, kind, seq + index, stamp, stamp,
                        json.dumps(data)))
        db.execute("UPDATE session_v2 SET time_updated = ?, title = coalesce(title, ?) WHERE id = ?",
                   (stamp, draft[:40], session))


def width():
    """Like OpenCode, lay the prompt out for the current terminal width."""
    try:
        return max(20, min(60, os.get_terminal_size(0).columns - 8))
    except OSError:
        return 60


def composer(draft, sent):
    rows = [f'  sent: {line}' for line in sent[-6:]]
    rows += ['', '    ┃', '    ┃  ' + (draft or 'Ask anything…'),
             '    ┃', '    ┃  Build · Fake-Model Free', '    ╹' + '▀' * width(),
             '    tab agents  ctrl+p commands']
    return rows, (len(rows) - 5, 7 + len(draft))


def palette():
    rows = ['', '        Commands                              esc', '',
            '        Search', '        Switch model                  ctrl+x m',
            '    ┃   Exit the app', '    ╹' + '▀' * width()]
    return rows, (4, 16)


def history(session):
    if not session or "SESSIONDOCK_TEST_OPENCODE_DB" not in os.environ:
        return []
    with database() as db:
        return [json.loads(data)["text"] for (data,) in db.execute(
            "SELECT data FROM session_message WHERE session_id = ? AND type = 'user' ORDER BY seq",
            (session,))]


def main():
    if sys.argv[1:2] == ["api"]:
        sys.exit(api(sys.argv[2:]))
    session = sys.argv[sys.argv.index("--session") + 1] if "--session" in sys.argv else None
    root = Path(os.environ['SESSIONDOCK_TEST_SCREEN'])
    old = termios.tcgetattr(0)
    tty.setraw(0)
    sys.stdout.write('\x1b[?2004h')
    # `draft` is what the prompt shows, `full` what OpenCode would store.
    draft, full, sent, previous, paste = '', '', history(session), None, None
    try:
        while True:
            state = root.read_text() if root.exists() else 'composer'
            rows, (y, x) = palette() if state == 'palette' else composer(draft, sent)
            frame = '\x1b[2J\x1b[H' + '\r\n'.join(rows) + f'\x1b[{y + 1};{x + 1}H'
            if frame != previous:
                sys.stdout.write(frame)
                sys.stdout.flush()
                previous = frame
            if not select.select([0], [], [], .02)[0]:
                continue
            data = os.read(0, 4096)
            if not data:
                break
            with root.with_suffix('.trace').open('ab') as trace:
                trace.write(data)
            text = data.decode('utf-8', 'replace')
            while text:
                if paste is not None:
                    end = text.find('\x1b[201~')
                    if end < 0:
                        paste += text
                        break
                    paste += text[:end]
                    text = text[end + 6:]
                    lines = paste.split('\n')
                    draft += paste if len(lines) < 3 else f'[Pasted ~{len(lines)} lines]'
                    full += paste
                    paste = None
                elif text.startswith('\x1b[200~'):
                    paste, text = '', text[6:]
                elif text[0] == '\r':
                    if draft and state != 'palette':
                        sent.append(draft)
                        if session and "SESSIONDOCK_TEST_OPENCODE_DB" in os.environ:
                            submit(session, full)
                        draft = full = ''
                    text = text[1:]
                elif text[0] == '\x7f':
                    draft, full, text = draft[:-1], full[:-1], text[1:]
                elif text[0] == '\x1b':
                    text = text[1:]
                else:
                    draft, full, text = draft + text[0], full + text[0], text[1:]
    finally:
        termios.tcsetattr(0, termios.TCSANOW, old)


if __name__ == '__main__':
    main()
