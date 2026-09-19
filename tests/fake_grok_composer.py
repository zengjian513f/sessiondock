#!/usr/bin/env python3
# run_validation: skip
"""Private, free Grok-shaped terminal; records every input byte for SEND tests."""
import os
from pathlib import Path
import select
import sys
import time
import termios
import tty

LOGIN = ('Approve in your browser to finish signing in.\n'
         'Make sure your browser shows this code.\nWaiting for approval...\nctrl+q  quit')
COMPOSER = ('  ╭────────────────────────────────────────────────────────╮\n'
            '  │ ❯                                                      │\n'
            '  ╰────────────── Grok 4.6 (low) · always-approve ───────────╯')


def main():
    root = Path(os.environ['SESSIONDOCK_TEST_SCREEN'])
    old = termios.tcgetattr(0)
    tty.setraw(0)
    sys.stdout.write('\x1b[?2004h')
    sys.stdout.flush()
    previous = None
    recover_at = None
    try:
        while True:
            if recover_at is not None and time.monotonic() >= recover_at:
                root.write_text('composer')
                recover_at = None
            state = root.read_text()
            if state != previous:
                text = COMPOSER if state == 'composer' else COMPOSER.replace('Grok 4.6 (low)', 'Custom engine') if state == 'custom' else LOGIN if state == 'login' else state
                sys.stdout.write('\x1b[2J\x1b[H' + text.replace('\n', '\r\n')
                                 + ('\x1b[2;7H' if state in ('composer', 'custom') else ''))
                sys.stdout.flush()
                previous = state
            if select.select([0], [], [], .02)[0]:
                data = os.read(0, 4096)
                if not data:
                    break
                with root.with_suffix('.trace').open('ab') as trace:
                    trace.write(data)
                if b'\x1b[200~' in data and root.with_suffix('.block').exists():
                    if root.with_suffix('.block').read_text() == 'wait':
                        root.write_text('')
                        recover_at = time.monotonic() + 1.2
                    else:
                        root.write_text('login')
    finally:
        termios.tcsetattr(0, termios.TCSANOW, old)


if __name__ == '__main__':
    main()
