"""Native-shaped Droid activity notifications through the existing isolated peer."""
# run_validation: skip
import builtins
import json
import os
import sys
from pathlib import Path
from droid_sdk_fixtures import DroidFixture, fake_cli, native_records, project_key

EVENTS = [
 {'type':'child_session_available','childSessionId':'60000000-0000-4000-8000-000000000099','description':'Inspect implementation','subagentType':'worker','timestamp':1794186000000},
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
 {'type':'tool_execution_heartbeat','toolUseId':'bg1','toolName':'Background shell','detached':True},
 {'type':'tool_execution_heartbeat','toolUseId':'bg1','toolName':'Background shell','detached':True},
 {'type':'tool_execution_phase_changed','toolUseId':'bg1','toolName':'Background shell','phase':'settled_after_execution'},
]
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
            child=EVENTS[0]['childSessionId']
            path=Path(os.environ['FACTORY_HOME_OVERRIDE'])/'.factory/sessions'/project_key(Path.cwd())/(child+'.jsonl')
            if not path.exists():
                rows=native_records(child,Path.cwd(),[('Child event prompt','Live child history answer')])
                rows[0].update(title='Live notified child',callingSessionId=frame['params']['sessionId'],callingToolUseId='activity-child-tool')
                path.write_text(''.join(json.dumps(row)+'\n' for row in rows))

            for event in EVENTS:
                original(json.dumps({'jsonrpc':'2.0','factoryApiVersion':'1.0.0','type':'notification','method':'droid.session_notification','params':{'sessionId':frame['params']['sessionId'],'notification':event}}),flush=True)
    builtins.print=emit
    fake_cli()
