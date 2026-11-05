"""Private browser fixture access to the durable conversation records."""
import json
import sqlite3
from pathlib import Path

SECTIONS = ('drafts', 'requests', 'uploads', 'legacy', 'aliases', 'reports', 'queued', 'merged_echoes')


def read_document(path):
    path = Path(path)
    database = path.with_suffix('.sqlite3')
    if not database.exists():
        return json.loads(path.read_text())
    with sqlite3.connect(f'file:{database}?mode=ro', uri=True) as db:
        document = {section: {} for section in SECTIONS}
        for section, key, data in db.execute('SELECT section,key,data FROM records'):
            document[section][key] = json.loads(data)
        return document


def write_document(path, document):
    """Replace private fixture state only while its server is stopped."""
    path = Path(path)
    database = path.with_suffix('.sqlite3')
    if not database.exists():
        path.write_text(json.dumps(document))
        return
    with sqlite3.connect(database) as db:
        db.execute('DELETE FROM records')
        db.executemany('INSERT INTO records VALUES(?,?,?)',
                       ((section, key, json.dumps(value)) for section in SECTIONS
                        for key, value in document.get(section, {}).items()))
