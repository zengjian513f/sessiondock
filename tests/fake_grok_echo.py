#!/usr/bin/env python3
# run_validation: skip
"""Free Grok-shaped terminal that publishes a timestamp-less chat_history.

The launcher passes ``--session-id``. This process only draws a composer the
readiness check accepts and creates ``summary.json`` plus an empty
``chat_history.jsonl`` under ``$SESSIONDOCK_GROK_ROOT``. The browser test
appends the user lines; this process never writes a ``timestamp`` field.
"""
import json
import os
from pathlib import Path
import sys
import termios
import tty

COMPOSER = ('  ╭────────────────────────────────────────────────────────╮\n'
            '  │ ❯                                                      │\n'
            '  ╰────────────── Grok 4.6 (low) · always-approve ───────────╯')


def session_id(argv):
    for index, arg in enumerate(argv):
        if arg == '--session-id' and index + 1 < len(argv):
            return argv[index + 1]
        if arg.startswith('--session-id='):
            return arg.split('=', 1)[1]
    return ''


def publish(sid):
    root = Path(os.environ['SESSIONDOCK_GROK_ROOT'])
    path = root / 'echo' / sid
    path.mkdir(parents=True, exist_ok=True)
    summary = path / 'summary.json'
    if not summary.exists():
        summary.write_text(json.dumps({
            'info': {'id': sid, 'cwd': os.environ.get('SESSIONDOCK_TEST_CWD', '/synthetic/echo')},
            'generated_title': 'echo',
            'current_model_id': 'grok-4.6',
        }), encoding='utf-8')
    chat = path / 'chat_history.jsonl'
    if not chat.exists():
        chat.write_bytes(b'')
    return path


def main():
    sid = session_id(sys.argv[1:])
    if not sid:
        raise SystemExit('fake grok echo requires --session-id')
    publish(sid)
    old = termios.tcgetattr(0)
    tty.setraw(0)
    try:
        sys.stdout.write('\x1b[?2004h\x1b[2J\x1b[H' + COMPOSER.replace('\n', '\r\n') + '\x1b[2;7H')
        sys.stdout.flush()
        while True:
            data = os.read(0, 4096)
            if not data:
                break
    finally:
        termios.tcsetattr(0, termios.TCSANOW, old)


if __name__ == '__main__':
    main()
