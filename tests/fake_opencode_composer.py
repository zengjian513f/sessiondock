#!/usr/bin/env python3
# run_validation: skip
"""Private, free OpenCode-shaped terminal; records every input byte for SEND tests.

The prompt block copies OpenCode 1.18.32: `┃` left border, a `╹▀…` rule, and
an `agent · model` row. `SESSIONDOCK_TEST_SCREEN` switches it: `composer`
(default) or `palette` (the command palette, which moves the cursor away).
A bracketed paste shows `[Pasted ~N lines]` like the real CLI; Enter echoes
the submitted draft above the prompt and empties it.
"""
import os
from pathlib import Path
import select
import sys
import termios
import tty


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


def main():
    root = Path(os.environ['SESSIONDOCK_TEST_SCREEN'])
    old = termios.tcgetattr(0)
    tty.setraw(0)
    sys.stdout.write('\x1b[?2004h')
    draft, sent, previous, paste = '', [], None, None
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
                    paste = None
                elif text.startswith('\x1b[200~'):
                    paste, text = '', text[6:]
                elif text[0] == '\r':
                    if draft and state != 'palette':
                        sent.append(draft)
                        draft = ''
                    text = text[1:]
                elif text[0] == '\x7f':
                    draft, text = draft[:-1], text[1:]
                elif text[0] == '\x1b':
                    text = text[1:]
                else:
                    draft, text = draft + text[0], text[1:]
    finally:
        termios.tcsetattr(0, termios.TCSANOW, old)


if __name__ == '__main__':
    main()
