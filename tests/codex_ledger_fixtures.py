"""Read/export private synthetic Codex ledgers, including migration fixtures."""
# run_validation: skip
from contextlib import closing
import json
import sqlite3


def queue_rows(root):
    path = root / 'state/codex-sdk-queue.sqlite'
    with closing(sqlite3.connect(path)) as db:
        rows = {}
        for scope, row in db.execute('SELECT scope,row FROM records ORDER BY rowid'):
            rows.setdefault(scope, []).append(json.loads(row))
        return rows


def legacy_queue(root):
    rows = queue_rows(root)
    path = root / 'state/codex-sdk-queue.sqlite'
    for suffix in ('', '-wal', '-shm'):
        path.with_name(path.name + suffix).unlink(missing_ok=True)
    legacy = root / 'state/codex-sdk-queue.json'
    legacy.write_text(json.dumps(rows))
    return legacy


def seed_completed(root, thread, count=5000):
    rows = [{'id':f'legacy-complete-{i}', 'text':'', 'input':[], 'model':None,
             'effort':None, 'state':'sent', 'error':None,
             'turn_id':f'legacy-turn-{i}', 'native_item_id':f'legacy-user-{i}'} for i in range(count)]
    (root/'state/codex-sdk-queue.json').write_text(json.dumps({thread:rows}))
