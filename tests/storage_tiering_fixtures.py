"""Read private fixtures across legacy JSON and tiered SQLite storage."""
import json
import gzip
import sqlite3
from pathlib import Path


def read_inputs(path):
    path = Path(path)
    db = path.with_suffix('.sqlite')
    if db.exists():
        with sqlite3.connect(f'{db.as_uri()}?mode=ro', uri=True) as connection:
            result = {}
            for scope, row in connection.execute('SELECT scope,row FROM inputs ORDER BY rowid'):
                result.setdefault(scope, []).append(json.loads(row))
            return result
    return json.loads(path.read_text()) if path.exists() else {}


def legacy_inputs(path, document):
    path = Path(path)
    for suffix in ('.sqlite', '.sqlite-wal', '.sqlite-shm'):
        path.with_suffix(suffix).unlink(missing_ok=True)
    path.write_text(json.dumps(document))


def checkpoints(directory):
    directory = Path(directory)
    return sorted({p.with_suffix('.json') for p in directory.glob('*.json')} |
                  {p.with_suffix('.json') for p in directory.glob('*.sqlite')})


def read_checkpoint(path):
    db = Path(path).with_suffix('.sqlite')
    if not db.exists():
        return json.loads(Path(path).read_text())
    with sqlite3.connect(f'{db.as_uri()}?mode=ro', uri=True) as connection:
        row = connection.execute('SELECT state FROM checkpoint WHERE id=1').fetchone()
        if row is None:
            return json.loads(Path(path).read_text())
        state = json.loads(row[0])
        def hydrate(state, key):
            owner = json.dumps(key, ensure_ascii=False, separators=(',', ':'))
            state['items'] = [json.loads(gzip.decompress(row[0]) if isinstance(row[0], bytes) else row[0]) for row in connection.execute(
                'SELECT value FROM items WHERE owner=? ORDER BY ordinal', (owner,))]
            for name, task in state.get('subtasks', {}).items():
                hydrate(task['state'], key + [name])
        hydrate(state, [])
        return state


def legacy_checkpoint(path, state):
    legacy_inputs(path, state)
