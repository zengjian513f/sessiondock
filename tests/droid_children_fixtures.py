"""Native Droid calling metadata fixtures; no CLI or provider activity required."""
# run_validation: skip
import json
import builtins
import os
from pathlib import Path
from droid_sdk_fixtures import DroidFixture, native_records, native_uid, project_key, fake_cli


class DroidChildrenFixture(DroidFixture):
    def __init__(self, root):
        super().__init__(root)
        launcher=json.loads(self.launcher.read_text())
        launcher["profiles"][0]["args"]=[str(Path(__file__).resolve()),"--fake-cli"]
        self.launcher.write_text(json.dumps(launcher))
        self.child = '60000000-0000-4000-8000-000000000001'
        self.fork = '60000000-0000-4000-8000-000000000002'
        self.orphan = '60000000-0000-4000-8000-000000000003'
        self.duplicate = '60000000-0000-4000-8000-000000000004'
        self.ambiguous = '60000000-0000-4000-8000-000000000005'
        self.intermediate = '60000000-0000-4000-8000-000000000006'
        self.nested = '60000000-0000-4000-8000-000000000007'
        self.paths = {}
        self.write(self.sid, 'Droid parent session', 'Parent history answer')
        self.write(self.child, 'Droid delegated child', 'Child history answer',
                   callingSessionId=self.sid, callingToolUseId='task-child')
        self.write(self.intermediate, 'Droid intermediate child', 'Intermediate child history', callingSessionId=self.sid)
        self.write(self.nested, 'Droid nested child', 'Nested child history', callingSessionId=self.intermediate)
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

    def process(self, pid, parent=1, sid=None):
        """Linux proc observation of an owned root or a delegated writer."""
        directory = self.root / 'proc' / str(pid)
        (directory / 'fd').mkdir(parents=True)
        (directory / 'stat').write_text(f"{pid} (droid) " + ' '.join(['S',str(parent),*(['0']*17),'100'])+'\n')
        argv=['droid','--resume',sid] if sid else ['droid','exec','--input-format','stream-jsonrpc','--output-format','stream-jsonrpc']
        (directory / 'cmdline').write_bytes(b'\0'.join(arg.encode() for arg in argv)+b'\0')
        (directory / 'environ').write_bytes(b'')
        (directory / 'cwd').symlink_to(self.cwd)
        return directory


if __name__ == '__main__':
    output=builtins.print
    def emit(*args,**kwargs):
        marker=Path(os.environ['SESSIONDOCK_FAKE_DROID_LOG']).parent/'missing-calling-parent'
        if marker.exists() and args and isinstance(args[0],str):
            try:
                frame=json.loads(args[0]);sid=frame.get('result',{}).get('sessionId')
                if sid:
                    path=Path(os.environ['FACTORY_HOME_OVERRIDE'])/'.factory/sessions'/project_key(Path.cwd())/(sid+'.jsonl')
                    rows=[json.loads(line) for line in path.read_text().splitlines()]
                    rows[0]['callingSessionId']=marker.read_text()
                    path.write_text(''.join(json.dumps(row)+'\n' for row in rows))
            except (ValueError,FileNotFoundError):pass
        output(*args,**kwargs)
    builtins.print=emit
    fake_cli()
