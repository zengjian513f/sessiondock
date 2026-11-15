"""Synthetic streaming peer and filesystem write counter for checkpoint pressure."""
# run_validation: skip
import asyncio
import ctypes
import json
import os
import struct

from codex_sdk_fixtures import FakeAppServer


class CheckpointPeer(FakeAppServer):
    async def _handle(self, writer, message):
        method = message.get('method')
        if method == 'thread/loaded/list':
            await self.send(writer, {'id': message['id'], 'result': {'data': [self.thread_id]}})
        elif method == 'thread/turns/list':
            await self.send(writer, {'id': message['id'], 'result': {'data': [], 'nextCursor': None}})
        else:
            await super()._handle(writer, message)

    async def burst(self, count):
        turn = self.turn
        item = {'type': 'agentMessage', 'id': 'checkpoint-stream', 'text': '', 'phase': 'commentary'}
        await self.notify('item/started', {'turnId': turn, 'item': item})
        for i in range(count):
            await self.notify('item/agentMessage/delta', {
                'turnId': turn, 'itemId': item['id'], 'delta': 'x'})
            if i % 100 == 0:
                await asyncio.sleep(0)
        # No completed-item replacement: the page must receive all deltas.
        await self.notify('item/agentMessage/delta', {
            'turnId': turn, 'itemId': item['id'], 'delta': ' checkpoint burst finished'})


class CheckpointWrites:
    """Count actual checkpoint replacements, including writes that share an mtime."""
    def __init__(self, path):
        self.path = path
        libc = ctypes.CDLL(None, use_errno=True)
        self.fd = libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC)
        if self.fd < 0:
            raise OSError(ctypes.get_errno(), 'inotify_init1')
        if libc.inotify_add_watch(self.fd, os.fsencode(path.parent), 0x80) < 0:
            self.close()
            raise OSError(ctypes.get_errno(), 'inotify_add_watch')

    def count(self):
        count = 0
        while True:
            try:
                data = os.read(self.fd, 65536)
            except BlockingIOError:
                return count
            offset = 0
            while offset < len(data):
                _, mask, _, length = struct.unpack_from('iIII', data, offset)
                name = data[offset + 16:offset + 16 + length].split(b'\0', 1)[0]
                assert not mask & 0x4000, 'checkpoint write counter overflowed'
                count += name == os.fsencode(self.path.name)
                offset += 16 + length

    def close(self):
        os.close(self.fd)


def legacy_outbox(frames):
    """Recreate the former durable ledger from requests actually sent by the browser."""
    entries = {}
    for frame in frames:
        if 'id' in frame:
            prefix = 'rpc' if 'method' in frame else 'reply'
            key = prefix + ':' + json.dumps(frame['id'], separators=(',', ':'))
        elif frame.get('method') == 'initialized':
            key = 'initialized'
        else:
            continue
        entries[key] = {'id': key, 'frame': frame}
    return list(entries.values())
