"""Native Droid calling metadata fixtures; no CLI or provider activity required."""
# run_validation: skip
import json
from droid_sdk_fixtures import DroidFixture, native_records, native_uid, project_key


class DroidChildrenFixture(DroidFixture):
    def __init__(self, root):
        super().__init__(root)
        self.child = '60000000-0000-4000-8000-000000000001'
        self.fork = '60000000-0000-4000-8000-000000000002'
        self.orphan = '60000000-0000-4000-8000-000000000003'
        self.duplicate = '60000000-0000-4000-8000-000000000004'
        self.ambiguous = '60000000-0000-4000-8000-000000000005'
        self.paths = {}
        self.write(self.sid, 'Droid parent session', 'Parent history answer')
        self.write(self.child, 'Droid delegated child', 'Child history answer',
                   callingSessionId=self.sid, callingToolUseId='task-child')
        self.write(self.fork, 'Droid independent fork', 'Fork history answer', parent=self.sid)
        self.write(self.orphan, 'Droid orphan child', 'Orphan hidden history',
                   callingSessionId='60000000-0000-4000-8000-999999999999')
        self.write(self.duplicate, 'Droid duplicate parent A', 'Parent copy A')
        other = self.cwd / 'duplicate-project'
        other.mkdir()
        self.write(self.duplicate, 'Droid duplicate parent B', 'Parent copy B', cwd=other)
        self.write(self.ambiguous, 'Droid ambiguous child', 'Ambiguous hidden history',
                   callingSessionId=self.duplicate)

    def write(self, sid, title, answer, cwd=None, **metadata):
        cwd = cwd or self.cwd
        path = self.projects / project_key(cwd) / (sid + '.jsonl')
        path.parent.mkdir(parents=True, exist_ok=True)
        rows = native_records(sid, cwd, [('Read this transcript', answer)])
        rows[0].update(title=title, **metadata)
        path.write_text(''.join(json.dumps(row) + '\n' for row in rows))
        self.paths[sid] = path
        return path

    def uid_for(self, sid):
        return native_uid(self.paths[sid])
