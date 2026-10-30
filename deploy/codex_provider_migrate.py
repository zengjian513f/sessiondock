#!/usr/bin/env python3
"""Explicit single-thread provider migration; dry run unless --apply is supplied.

Run only for an authorized provider migration between compatible backends.
Native writer coordination, backups and SQLite transactions preserve other
threads. This is maintenance, not an automatic SDK-open fallback.
"""
from __future__ import annotations

import argparse
from bisect import bisect_right
from contextlib import ExitStack
import fcntl
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import tempfile


def replace_field(line, field, old, new):
    pattern = rb'(?<!\\)"' + field.encode() + rb'"\s*:\s*("(?:[^"\\]|\\.)*")'
    matches = list(re.finditer(pattern, line))
    if len(matches) != 1 or json.loads(matches[0][1]) != old:
        raise ValueError(f'expected one {field} with the source provider')
    start, end = matches[0].span(1)
    return line[:start] + json.dumps(new, ensure_ascii=False).encode() + line[end:]


def migrate(home, thread, source, destination, backup, apply=False):
    home, backup = Path(home), Path(backup)
    with ExitStack() as stack:
        if apply:
            coordination = stack.enter_context((home/'thread-writer-locks/.coordination.lock').open('a+b'))
            fcntl.flock(coordination, fcntl.LOCK_EX | fcntl.LOCK_NB)
            lock = home/'thread-writer-locks'/f'{thread}.lock'
            if lock.exists():
                writer = stack.enter_context(lock.open('r+b'))
                fcntl.flock(writer, fcntl.LOCK_EX | fcntl.LOCK_NB)
        mode = 'rw' if apply else 'ro'
        db = sqlite3.connect(f'file:{home / "state_5.sqlite"}?mode={mode}', uri=True, timeout=10)
        stack.callback(db.close)
        db.row_factory = sqlite3.Row
        history = home/'thread_history_1.sqlite'
        if history.exists():
            db.execute('ATTACH DATABASE ? AS history', (f'file:{history}?mode={mode}',))
        if apply:
            db.execute('BEGIN IMMEDIATE')
        row = db.execute('SELECT * FROM threads WHERE id=?', (thread,)).fetchone()
        if row is None or row['model_provider'] != source:
            raise ValueError('thread is missing or its provider differs from the requested source')
        path = Path(row['rollout_path'])
        original = path.read_bytes()
        lines = original.splitlines(keepends=True)
        meta = json.loads(lines[0])
        if meta['type'] != 'session_meta' or meta['payload']['id'] != thread:
            raise ValueError('rollout identity does not match the selected thread')
        edits = {0: 'model_provider'}
        # Keep historical settings as history; update the latest owned settings
        # snapshot used to restore current configuration and rebuild metadata.
        latest = None
        for n, line in enumerate(lines):
            value = json.loads(line)
            payload = value.get('payload', {})
            if (value.get('type') == 'event_msg' and payload.get('type') == 'thread_settings_applied'
                    and payload.get('thread_id') == thread):
                latest = n
        if latest is not None:
            edits[latest] = 'model_provider_id'
        boundaries, deltas = [], []
        offset, total = 0, 0
        for n, line in enumerate(lines):
            offset += len(line)
            if n in edits:
                lines[n] = replace_field(line, edits[n], source, destination)
                total += len(lines[n]) - len(line)
                boundaries.append(offset)
                deltas.append(total)
        def shifted(value):
            if value is None:
                return None
            i = bisect_right(boundaries, value)
            return value + (deltas[i-1] if i else 0)
        turns = [dict(r) for r in db.execute(
            'SELECT turn_id,rollout_byte_offset,rollout_end_byte_offset FROM history.thread_turns WHERE thread_id=?', (thread,))] if history.exists() else []
        projection = db.execute('SELECT * FROM history.thread_history_projection_state WHERE thread_id=?', (thread,)).fetchone() if history.exists() else None
        result = {'thread_id': thread, 'from': source, 'to': destination,
                  'record_lines': [n+1 for n in edits], 'size_delta': total, 'indexed_turns': len(turns), 'applied': False}
        if not apply:
            return result
        backup.mkdir(mode=0o700, parents=True, exist_ok=False)
        shutil.copy2(path, backup/'rollout.jsonl')
        (backup/'metadata.json').write_text(json.dumps({'thread': dict(row), 'turn_offsets': turns,
            'projection': dict(projection) if projection else None, 'migration': result}, ensure_ascii=False, indent=2))
        fd, temporary = tempfile.mkstemp(prefix=path.name+'.provider-', dir=path.parent)
        swapped = False
        try:
            with os.fdopen(fd, 'wb') as stream:
                stream.write(b''.join(lines)); stream.flush(); os.fsync(stream.fileno())
            shutil.copystat(path, temporary)
            db.execute('UPDATE threads SET model_provider=? WHERE id=?', (destination, thread))
            for turn in turns:
                db.execute('UPDATE history.thread_turns SET rollout_byte_offset=?,rollout_end_byte_offset=? WHERE thread_id=? AND turn_id=?',
                    (shifted(turn['rollout_byte_offset']), shifted(turn['rollout_end_byte_offset']), thread, turn['turn_id']))
            if projection:
                db.execute('UPDATE history.thread_history_projection_state SET next_rollout_byte_offset=? WHERE thread_id=?',
                           (shifted(projection['next_rollout_byte_offset']), thread))
            os.replace(temporary, path); swapped = True
            db.commit()
        except BaseException:
            db.rollback()
            if swapped:
                shutil.copy2(backup/'rollout.jsonl', path)
            raise
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)
        result['applied'] = True
        result['backup'] = str(backup)
        return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--home', type=Path, required=True)
    parser.add_argument('--thread', required=True)
    parser.add_argument('--from-provider', required=True)
    parser.add_argument('--to-provider', required=True)
    parser.add_argument('--backup', type=Path, required=True)
    parser.add_argument('--apply', action='store_true')
    args = parser.parse_args()
    print(json.dumps(migrate(args.home, args.thread, args.from_provider, args.to_provider, args.backup, args.apply), ensure_ascii=False))


if __name__ == '__main__':
    main()
