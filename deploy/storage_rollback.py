"""Export tiered state for an older binary while the node service is stopped.

Used only by rollback; normal deployment never reads these bodies. Input and
adapter identities, pending RPCs, cursors and native files remain unchanged.
"""
from pathlib import Path
from contextlib import closing
import gzip
import json
import os
import shutil
import sqlite3
import sys


def write(path, value):
    temporary = path.with_suffix('.rollback-tmp')
    with open(temporary, 'w', encoding='utf-8') as stream:
        os.chmod(temporary, 0o600)
        json.dump(value, stream, ensure_ascii=False, separators=(',', ':'))
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)
    if os.name != 'nt':
        fd = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)


def remove_database(path):
    for suffix in ('', '-wal', '-shm'):
        Path(str(path) + suffix).unlink(missing_ok=True)


def host_alive(directory):
    try:
        pid = int((directory / 'host.pid').read_text().strip())
    except (OSError, ValueError):
        return True  # Missing ownership evidence is not proof of host exit.
    if os.name == 'nt':
        import ctypes
        from ctypes import wintypes
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.OpenProcess.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.GetExitCodeProcess.argtypes = (wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD))
        kernel.CloseHandle.argtypes = (wintypes.HANDLE,)
        handle = kernel.OpenProcess(0x1000, False, pid)
        if not handle:
            return ctypes.get_last_error() != 87  # invalid PID; access errors remain unknown
        try:
            code = wintypes.DWORD()
            return not kernel.GetExitCodeProcess(handle, ctypes.byref(code)) or code.value == 259
        finally:
            kernel.CloseHandle(handle)
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def export_abandoned_streams(root):
    # Live newer hosts still serve older clients through the unchanged Read RPC
    # and produce a legacy-compatible gzip on exit. Do not touch their segments.
    for parts in (root / 'sdkhost').glob('*/*.parts'):
        target = parts.with_suffix('')
        if target.with_suffix('.gz').exists() or host_alive(parts.parent):
            continue
        pieces = {}
        for path in parts.iterdir():
            if path.suffix in ('.raw', '.gz') and path.stem.isdigit():
                offset = int(path.stem)
                if path.suffix == '.gz' or offset not in pieces:
                    pieces[offset] = path
        if not pieces:
            continue
        temporary = target.with_suffix('.rollback-tmp')
        with open(temporary, 'wb') as out:
            os.chmod(temporary, 0o600)
            for offset, path in sorted(pieces.items()):
                if out.tell() != offset:
                    raise ValueError('incomplete SDK output segments')
                with (gzip.open(path, 'rb') if path.suffix == '.gz' else open(path, 'rb')) as incoming:
                    shutil.copyfileobj(incoming, out)
            out.flush()
            os.fsync(out.fileno())
        os.replace(temporary, target)
        if os.name != 'nt':
            fd = os.open(target.parent, os.O_RDONLY)
            try:
                os.fsync(fd)
            finally:
                os.close(fd)


def export(root):
    path = root / 'native-sdk-inputs.sqlite'
    if path.exists():
        rows = {}
        with closing(sqlite3.connect(f'{path.as_uri()}?mode=ro', uri=True)) as db:
            for scope, row in db.execute('SELECT scope,row FROM inputs ORDER BY rowid'):
                rows.setdefault(scope, []).append(json.loads(row))
        write(path.with_suffix('.json'), rows)
        remove_database(path)
    for name in ('codex-sdk-queue', 'codex-sdk-sessions'):
        path = root / (name + '.sqlite')
        if not path.exists():
            continue
        rows = {}
        with closing(sqlite3.connect(f'{path.as_uri()}?mode=ro', uri=True)) as db:
            for scope, key, row in db.execute('SELECT scope,id,row FROM records ORDER BY rowid'):
                if name == 'codex-sdk-queue':
                    rows.setdefault(scope, []).append(json.loads(row))
                else:
                    rows[key] = json.loads(row)
        write(path.with_suffix('.json'), rows)
        remove_database(path)
    for path in (root / 'sdk-adapters/kiro').glob('*.sqlite'):
        with closing(sqlite3.connect(f'{path.as_uri()}?mode=ro', uri=True)) as db:
            row = db.execute('SELECT state FROM checkpoint WHERE id=1').fetchone()
            if row is None:
                continue
            state = json.loads(row[0])
            def hydrate(state, key):
                owner = json.dumps(key, ensure_ascii=False, separators=(',', ':'))
                state['items'] = [json.loads(gzip.decompress(row[0]) if isinstance(row[0], bytes) else row[0]) for row in db.execute(
                    'SELECT value FROM items WHERE owner=? ORDER BY ordinal', (owner,))]
                for name, task in state.get('subtasks', {}).items():
                    hydrate(task['state'], key + [name])
            hydrate(state, [])
        write(path.with_suffix('.json'), state)
        remove_database(path)
    export_abandoned_streams(root)
    for path in (root / 'sdkhost').glob('*/*.gz'):
        if path.name not in ('stdout.gz', 'stderr.gz'):
            continue
        target = path.with_suffix('')
        if target.exists():
            continue
        temporary = target.with_suffix('.rollback-tmp')
        with gzip.open(path, 'rb') as source, open(temporary, 'wb') as out:
            os.chmod(temporary, 0o600)
            shutil.copyfileobj(source, out)
            out.flush()
            os.fsync(out.fileno())
        os.replace(temporary, target)


if __name__ == '__main__':
    export(Path(sys.argv[1]).resolve())
