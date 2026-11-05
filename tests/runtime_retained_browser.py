#!/usr/bin/env python3
"""A browser-used node must scan retained logs without creating rewrite files."""
import ctypes
import json
import os
from pathlib import Path
import struct
import tempfile
import time
from playwright.sync_api import sync_playwright
from history_fixtures import BINARY, build_corpus, isolated_server
from runtime_layers_browser import until


def main():
    with tempfile.TemporaryDirectory(prefix='sd-retained-') as temporary, sync_playwright() as pw:
        root = Path(temporary)
        corpus = build_corpus(root)
        logs = root / 'runtime'
        logs.mkdir()
        retained = logs / 'runtime-999999-1.jsonl'
        retained.write_text((json.dumps({'timestamp_ms': int(time.time()*1000), 'event': 'retained-fixture'})+'\n')*50000)
        before = retained.stat()
        libc = ctypes.CDLL(None, use_errno=True)
        fd = libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC)
        assert fd >= 0
        assert libc.inotify_add_watch(fd, os.fsencode(logs), 0x100) >= 0
        try:
            for restart in range(2):
                with isolated_server(corpus, BINARY, extra_env={'SESSIONDOCK_RUNTIME_LOG_DIR': str(logs)}) as (base, _):
                    browser = pw.chromium.launch(headless=True)
                    page = browser.new_page()
                    page.goto(base)
                    page.locator(f'#side .item[data-uid="{corpus.uid("codex-fork")}"]').click()
                    until(page, lambda: page.request.get(base+'/api/health').json()['runtime_log']['cleanup_scanned_bytes'] >= before.st_size)
                    assert retained.stat().st_mtime_ns == before.st_mtime_ns
                    names = []
                    while True:
                        try: events = os.read(fd, 65536)
                        except BlockingIOError: break
                        offset = 0
                        while offset < len(events):
                            _, _, _, size = struct.unpack_from('iIII', events, offset)
                            names.append(events[offset+16:offset+16+size].rstrip(b'\0').decode())
                            offset += 16+size
                    assert not any(name.startswith('.runtime-clean-') for name in names), names
                    browser.close()
        finally:
            os.close(fd)
    print('PASS retained log maintenance: browser use, two starts, no temporary rewrite files')

if __name__ == '__main__': main()
