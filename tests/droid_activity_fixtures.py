"""Native-shaped Droid activity notifications through the existing isolated peer."""
# run_validation: skip
import builtins
import json
import os
import sys
import time
from pathlib import Path
from droid_sdk_fixtures import DroidFixture, fake_cli, native_records, project_key

CHILD_SID = '60000000-0000-4000-8000-000000000099'

EVENTS = [
 {'type':'child_session_available','childSessionId':CHILD_SID,'description':'Inspect implementation','subagentType':'worker','timestamp':1794186000000},
 {'type':'hook_execution_started','hookId':'h1','hookEventName':'PreToolUse','hookCommands':[]},
 {'type':'hook_execution_completed','hookId':'h1','hookEventName':'PreToolUse','hookStatus':'completed','hookResults':[{'exitCode':0,'stdout':'Hook visible output','stderr':''},{'exitCode':0,'stdout':'SUPPRESSED_SECRET','stderr':'','suppressOutput':True}]},
 {'type':'hook_execution_started','hookId':'hidden','hookEventName':'HIDDEN_HOOK','hookCommands':[],'hiddenFromUserViews':True},
 {'type':'hook_execution_completed','hookId':'hidden','hookStatus':'completed'},
 {'type':'mcp_status_changed','servers':[{'name':'isolated-mcp','status':'connected','toolCount':2,'pendingAuthState':'AUTH_SECRET'}],'summary':{'total':1,'connected':1,'connecting':0,'failed':0}},
 {'type':'mission_state_changed','state':'running'},
 {'type':'mission_features_changed','features':[{'id':'feature-1','description':'Inspect isolated feature','status':'completed'}]},
 {'type':'mission_progress_entry','progressLog':[{'timestamp':'2026-11-09T01:00:00Z','type':'mission_accepted','title':'Isolated mission accepted'}]},
 {'type':'mission_worker_started','workerSessionId':'mission-worker'},
 {'type':'mission_worker_completed','workerSessionId':'mission-worker','exitCode':0},
 {'type':'mission_heartbeat','timestamp':'2026-11-09T01:00:00Z'},
]


def tool_call(tool_id, command):
    return {'type':'tool_call','toolUse':{'type':'tool_use','id':tool_id,
        'name':'Execute','input':{'command':command}}}


def heartbeat(tool_id, detached=True):
    return {'type':'tool_execution_heartbeat','toolUseId':tool_id,'toolName':'Execute','detached':detached}


def output(tool_id, text):
    return {'type':'tool_progress_update','toolUseId':tool_id,'toolName':'Execute',
        'update':{'type':'tool_result','terminalId':'synthetic-terminal','fullOutput':text}}


def settle(tool_id):
    return {'type':'tool_execution_phase_changed','toolUseId':tool_id,'toolName':'Execute',
        'phase':'settled_after_execution'}


class DroidActivityFixture(DroidFixture):
    def __init__(self, root):
        super().__init__(root)
        launcher=json.loads(self.launcher.read_text())
        launcher['profiles'][0]['args']=[str(Path(__file__).resolve()),'--fake-cli']
        self.launcher.write_text(json.dumps(launcher))

if __name__ == '__main__':
    original=builtins.print
    def emit(*args, **kwargs):
        original(*args, **kwargs)
        if len(args) != 1 or not isinstance(args[0],str): return
        try: frame=json.loads(args[0])
        except ValueError: return
        if frame.get('params',{}).get('notification',{}).get('type') == 'agent_turn_completed':
            sid=frame['params']['sessionId']
            path=Path(os.environ['FACTORY_HOME_OVERRIDE'])/'.factory/sessions'/project_key(Path.cwd())/(CHILD_SID+'.jsonl')
            if not path.exists():
                rows=native_records(CHILD_SID,Path.cwd(),[('Child event prompt','Live child history answer')])
                rows[0].update(title='Live notified child',callingSessionId=sid,callingToolUseId='activity-child-tool')
                path.write_text(''.join(json.dumps(row)+'\n' for row in rows))

            def notify(owner, event):
                original(json.dumps({'jsonrpc':'2.0','factoryApiVersion':'1.0.0','type':'notification',
                    'method':'droid.session_notification','params':{'sessionId':owner,'notification':event}}),flush=True)
            for event in EVENTS:
                notify(sid, event)
            notify(sid, tool_call('foreground', 'synthetic foreground'))
            notify(sid, heartbeat('foreground', detached=False))
            notify(sid, output('foreground', 'FOREGROUND_OUTPUT'))
            notify(sid, settle('foreground'))
            for owner, command, full in [(sid, 'synthetic parent shell', 'early prefix\n' + '日' * 22000 + '\nDroid own tail'),
                                         (CHILD_SID, 'synthetic child shell', 'Droid child tail')]:
                notify(owner, tool_call('bg1', command))
                if owner == sid:
                    notify(owner, heartbeat('bg1'))
                notify(owner, output('bg1', full))
                notify(owner, heartbeat('bg1'))
            notify('unbound-synthetic-sid', heartbeat('unbound'))
            notify('unbound-synthetic-sid', output('unbound', 'UNBOUND_OUTPUT'))
            gate = Path(os.environ['SESSIONDOCK_FAKE_DROID_LOG']).parent / 'settle-background'
            deadline = time.monotonic() + 40
            while not gate.exists() and time.monotonic() < deadline:
                time.sleep(.025)
            notify(sid, settle('bg1'))
            notify(sid, heartbeat('bg1'))
            notify(CHILD_SID, settle('bg1'))
    builtins.print=emit
    fake_cli()
