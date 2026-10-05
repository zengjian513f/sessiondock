#!/usr/bin/env python3
# run_validation: skip
"""Fake CLI startup choices controlled by a private fixture file; never a model."""
import os
import sys
import termios
import tty
from pathlib import Path
from fake_claude_cli import Fake, parse

class ConversationFake(Fake):
    """Claude 2.1.289 rejects a misspelled command only in the terminal."""
    def write(self, text):
        super().write(text.replace('> Unknown command:', 'Unknown command:'))

    def submit(self):
        if self.buffer == '/mode':
            self.buffer = ''
            self.collapsed_paste = None
            control = os.environ.get('SESSIONDOCK_TEST_COMMAND_RESPONSE', '')
            mode = Path(control).read_text() if control and Path(control).exists() else ''
            if mode != 'swallow':
                warning = 'Unknown command: /mode. Did you mean /model?'
                self.transcript.append('quoted: ' + warning if mode == 'quote' else warning)
            self.render()
            return
        super().submit()

def main():
    gate = Path(os.environ['SESSIONDOCK_TEST_GATE'])
    trace = Path(os.environ['SESSIONDOCK_TEST_GATE_TRACE'])
    if gate.exists():
        previous = termios.tcgetattr(0)
        try:
            tty.setraw(0)
            sys.stdout.write('\x1b[2J\x1b[H' + gate.read_text().replace('\n','\r\n'))
            sys.stdout.flush()
            while True:
                data = os.read(0,4096)
                with trace.open('ab') as file:
                    file.write(data)
                if b'\r' in data or b'\n' in data:
                    gate.unlink(missing_ok=True)
                    break
        finally:
            termios.tcsetattr(0,termios.TCSANOW,previous)
    ConversationFake(parse(sys.argv[1:])).run()

if __name__ == '__main__':
    main()
