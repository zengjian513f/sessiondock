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
        elif method == 'thread/turns/list' and str(message.get('params',{}).get('cursor','')).startswith('duplex-'):
            await self.send(writer, {'id':message['id'],'result':{'data':[], 'nextCursor':None,'padding':'p'*(2*1024*1024)}})
        elif method == 'thread/turns/list' and str(message.get('params',{}).get('cursor','')).startswith('sustained-'):
            async def later():
                await asyncio.sleep(2)
                await self.send(writer, {'id':message['id'],'result':{'data':[], 'nextCursor':None}})
            asyncio.create_task(later())
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

    async def large_frame(self):
        await self.notify('thread/status/changed', {'status': {'type':'active'}, 'padding':'z'*(10*1024*1024)})
        await self.notify('item/agentMessage/delta', {'turnId':self.turn,
            'itemId':'checkpoint-stream','delta':' large-frame-finished'})

    async def disconnect_queries(self):
        for writer in list(self.query_clients):
            writer.close()
            await writer.wait_closed()


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
        if str(frame.get('id','')).startswith('query:'):
            continue
        if 'id' in frame:
            prefix = 'rpc' if 'method' in frame else 'reply'
            key = prefix + ':' + json.dumps(frame['id'], separators=(',', ':'))
        elif frame.get('method') == 'initialized':
            key = 'initialized'
        else:
            continue
        entries[key] = {'id': key, 'frame': frame}
    return list(entries.values())


def seed_legacy_reads(root, native, count=500):
    """Represent an older service that durably queued repeatable read RPCs."""
    import socket
    directory, = (root/'state/sdkhost').glob('codex-channel-*')
    fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
    try:
        for i in range(count):
            frame = {'id': 10000+i, 'method': 'thread/turns/list', 'params': {
                'threadId': native.thread_id, 'cursor': str(i)+'c'*8192}}
            request = {'Write': {'id': f'rpc:{frame["id"]}',
                                'bytes': list((json.dumps(json.dumps(frame))+'\n').encode())}}
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
                client.settimeout(10)
                client.connect(f'/proc/self/fd/{fd}/control.sock')
                client.sendall(json.dumps(request).encode()+b'\n')
                reply = json.loads(client.makefile('rb').readline())
                assert reply['value']['state'] == 'written', reply
        native.wait_for(lambda: any(m.get('id') == 10000+count-1 for m in native.received))
    finally:
        os.close(fd)


def owner_initializations(native):
    return [m for m in native.methods('initialize') if m['params']['clientInfo']['name'] == 'sessiondock']
