"""Noncanonical native JSON layouts and exact identity-only byte assertions."""
import json
from pathlib import Path


def varied_layout(root):
    for path in root.rglob('*'):
        if path.is_symlink() or path.suffix not in ('.json', '.jsonl') or not path.is_file():
            continue
        try:
            if path.suffix == '.json':
                value=json.loads(path.read_bytes())
                raw=(' \t'+json.dumps(value,ensure_ascii=True,indent=3)+'  ').encode()
            else:
                rows=[json.loads(line) for line in path.read_bytes().splitlines() if line.strip()]
                for row in rows:
                    row['fixture_literal']='café / Ω 🐈'
                    row['fixture_number']=20.0
                    # Exercise nested tool JSON while leaving user text alone.
                    for call in row.get('tool_calls',[]):
                        if isinstance(call.get('arguments'),str):
                            call['arguments']='  '+json.dumps(json.loads(call['arguments']),indent=2)+'  '
                raw=b'\r\n'.join((' \t'+json.dumps(row,ensure_ascii=True,separators=(',  ', ' : '))+' \t').encode() for row in rows)+b'\r\n'
                raw=raw.replace(b'"fixture_number" : 20.0',b'"fixture_number" : 2.00e1').replace(b' / ',b' \\/ ')
            path.write_bytes(raw)
        except (ValueError,UnicodeError):
            continue


def assert_identity_only(operation, originals):
    plan=operation['file_plan']
    mapping={key.split(':',1)[1]:value for table in ('sessions','records') for key,value in plan[table].items()}
    for publication in operation['file_publications']:
        original=originals[publication['source']]
        raw=Path(publication['staging']).read_bytes()
        for old,new in sorted(mapping.items(),key=lambda item:len(item[1]),reverse=True):
            raw=raw.replace(new.encode(),old.encode())
            raw=raw.replace(('Resuming agent '+new[:7]).encode(),('Resuming agent '+old[:7]).encode())
        # The fixture's short SendMessage destination is intentionally resolved
        # to the full identity before applying the clone's identity mapping.
        if publication['source'].endswith('.jsonl'):
            for old in plan['sessions']:
                if old.startswith('claude:a'):
                    agent=old.split(':',1)[1]
                    original=original.replace(('"to" : "'+agent[:7]+'"').encode(),('"to" : "'+agent+'"').encode())
        assert raw==original, ('non-identity bytes changed',publication['source'])
