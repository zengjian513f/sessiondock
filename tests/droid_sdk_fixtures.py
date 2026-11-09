"""Synthetic Factory Droid 1.6 JSON-RPC peer; no provider calls or real CLI homes."""
# run_validation: skip
from pathlib import Path
import json
import os
import sys
import time
import uuid
from contextlib import contextmanager
from codebuddy_sdk_fixtures import CodebuddyFixture, native_uid as buddy_uid


def project_key(cwd):
    return str(cwd).rstrip('/').replace('/', '-')


def native_uid(path):
    return buddy_uid(path).replace('codebuddy:', 'droid:', 1)


def native_records(sid, cwd, turns):
    rows = [{'type':'session_start','id':sid,'title':'Droid isolated browser session','owner':'synthetic','version':2,'cwd':str(cwd)}]
    for i,(q,a) in enumerate(turns):
        for role,text in [('user',q),('assistant',a)]:
            rows.append({'type':'message','id':f'seed-{role}-{i}','timestamp':'2026-11-09T01:00:00Z','message':{'role':role,'content':[{'type':'text','text':text}], 'modelId':'fake-droid'}})
    return rows


class DroidFixture(CodebuddyFixture):
    def __init__(self, root):
        super().__init__(root)
        self.config = self.home / '.factory'
        self.projects = self.config / 'sessions'
        self.path = self.projects / project_key(self.cwd) / (self.sid + '.jsonl')
        self.path.parent.mkdir(parents=True)
        self.path.write_text(''.join(json.dumps(r)+'\n' for r in native_records(self.sid,self.cwd,[('earlier Droid prompt','earlier Droid answer')])) )
        self.uid = native_uid(self.path)
        self.received = self.root / 'droid-received.jsonl'
        launcher=json.loads(self.launcher.read_text())
        profile=launcher['profiles'][0]
        profile.update(id='droid-fixture',source='droid',args=[str(Path(__file__).resolve()),'--fake-cli'],env={'HOME':str(self.home),'FACTORY_HOME_OVERRIDE':str(self.home),'SESSIONDOCK_FAKE_DROID_LOG':str(self.received)})
        self.launcher.write_text(json.dumps(launcher))

    @contextmanager
    def server(self,binary,extra_env=None,**kwargs):
        with super().server(binary,{'SESSIONDOCK_CODEBUDDY_ROOT':'','SESSIONDOCK_DROID_ROOT':str(self.projects),**(extra_env or {})},**kwargs) as base:
            yield base


def fake_cli():
    if '--version' in sys.argv: print('1.6.0');return
    if '--help' in sys.argv: print('Synthetic Droid help');return
    if 'exec' not in sys.argv[2:]:
        with Path(os.environ['SESSIONDOCK_FAKE_DROID_LOG']).open('a') as f:
            f.write(json.dumps({'type':'cli_start','args':sys.argv[2:],'cwd':str(Path.cwd())})+'\n')
        print('Droid fake terminal ready',flush=True)
        for line in sys.stdin:print('Droid CLI echoed: '+line.strip(),flush=True)
        return
    assert sys.argv[-5:] == ['exec','--input-format','stream-jsonrpc','--output-format','stream-jsonrpc'],sys.argv
    log=Path(os.environ['SESSIONDOCK_FAKE_DROID_LOG'])
    def record(v):
        with log.open('a') as f:f.write(json.dumps(v)+'\n')
    record({'type':'spawn','args':sys.argv[2:],'pid':os.getpid()})
    sid=None;path=None;pending={};context_used=12000;settings={'modelId':'fake-droid','reasoningEffort':'low','interactionMode':'auto','autonomyLevel':'off'}
    models=[{'id':m,'displayName':name,'supportedReasoningEfforts':['low','high']} for m,name in [('fake-droid','Synthetic Droid'),('fake-droid-2','Synthetic Droid 2')]]
    def emit(v):print(json.dumps({'jsonrpc':'2.0','factoryApiVersion':'1.0.0',**v}),flush=True)
    def reply(v,result):emit({'id':v['id'],'type':'response','result':result})
    def note(v):emit({'type':'notification','method':'droid.session_notification','params':{'sessionId':sid,'notification':v}})
    def msg(mid,role,text,request=None):
        m={'id':mid,'role':role,'content':[{'type':'text','text':text}],'createdAt':1794186000000,'modelId':settings['modelId']}
        with path.open('a') as f:f.write(json.dumps({'type':'message','id':mid,'timestamp':'2026-11-09T01:00:00Z','message':m})+'\n')
        note({'type':'create_message','message':m,**({'requestId':request} if request else {})})
    def finish(rid,text):
        msg('answer-'+rid,'assistant','Droid final '+text)
        note({'type':'session_token_usage_changed','tokenUsage':{'inputTokens':1200,'outputTokens':50,'thinkingTokens':20,'cacheReadTokens':300,'cacheCreationTokens':40,'factoryCredits':1.25}})
        note({'type':'agent_turn_completed','reason':'completed','tokenUsage':{'inputTokens':10,'outputTokens':5}})
    for line in sys.stdin:
        v=json.loads(line);record(v);method=v.get('method');p=v.get('params',{})
        if not method:
            request=pending.pop(v['id']);finish(request['id'],request['params']['text']);continue
        if method=='droid.list_models':reply(v,{'models':models})
        elif method in ('droid.initialize_session','droid.load_session'):
            sid=p['sessionId'];path=Path(os.environ['FACTORY_HOME_OVERRIDE'])/'.factory/sessions'/project_key(Path.cwd())/(sid+'.jsonl')
            if method=='droid.initialize_session':
                path.parent.mkdir(parents=True,exist_ok=True);path.write_text(json.dumps(native_records(sid,Path.cwd(),[])[0])+'\n');settings.update({k:p[k] for k in ('modelId','reasoningEffort') if k in p})
            else:assert path.is_file()
            reply(v,{'sessionId':sid,'session':{'messages':[]},'settings':settings,'availableModels':models})
        elif method=='droid.rename_session':
            while (log.parent/'rename-hold').exists():time.sleep(.02)
            if p['title']=='reject rename':
                emit({'id':v['id'],'error':{'code':-32000,'message':'Synthetic rename failure'}});continue
            rows=[json.loads(line) for line in path.read_text().splitlines()]
            rows[0].update(title=p['title'],isSessionTitleManuallySet=True)
            path.write_text(''.join(json.dumps(row)+'\n' for row in rows))
            reply(v,{'success':True});note({'type':'session_title_updated','title':p['title'],'updateType':'manual_rename'})
        elif method=='droid.fork_session':
            if (log.parent/'fork-fail').exists():
                emit({'id':v['id'],'error':{'code':-32000,'message':'Synthetic fork failure'}});continue
            child=str(uuid.uuid4())
            rows=[json.loads(line) for line in path.read_text().splitlines()]
            messages=[row for row in rows if row.get('type')=='message']
            rows[0].update(id=child,parent=sid,title=p.get('title','Droid native fork'),forkedAtMessageId=messages[-1]['id'] if messages else None)
            path.with_name(child+'.jsonl').write_text(''.join(json.dumps(row)+'\n' for row in rows))
            reply(v,{'newSessionId':child})
        elif method=='droid.compact_session':
            context_used=8000
            child=sid
            if (log.parent/'compact-new-session').exists():
                child=str(uuid.uuid4())
                rows=native_records(child,Path.cwd(),[('Native compact summary','Earlier conversation preserved as a compact summary')])
                rows[0].update(parent=sid,title='Droid compact continuation')
                path.with_name(child+'.jsonl').write_text(''.join(json.dumps(row)+'\n' for row in rows))
            else:
                with path.open('a') as f:f.write(json.dumps({'type':'compaction_state','removedCount':3})+'\n')
            reply(v,{'newSessionId':child,'removedCount':3})
            note({'type':'session_compacted','summaryId':'synthetic-summary','removedCount':3,'visibleBoundaryMessageId':None})
        elif method=='droid.get_context_stats':
            if (log.parent/'context-fail').exists():emit({'id':v['id'],'error':{'code':-32601,'message':'Unavailable context'}})
            else:reply(v,{'used':context_used,'remaining':150000-context_used,'limit':150000,'accuracy':'estimated','updatedAt':'2026-11-09T01:00:00Z'})
        elif method=='droid.update_session_settings':
            if (log.parent/'settings-fail').exists():
                emit({'id':v['id'],'error':{'code':-32000,'message':'Synthetic settings failure'}});continue
            settings.update(p);reply(v,{});note({'type':'settings_updated','settings':settings})
        elif method=='droid.add_user_message':
            msg(p['messageId'],'user',p['text'],v['id']);reply(v,{})
            if p['text']=='hold':note({'type':'assistant_text_delta','messageId':'held','blockIndex':0,'textDelta':'Droid working'})
            elif p['text']=='tool':
                tool={'type':'tool_use','id':'tool-example','name':'Read','input':{'path':'synthetic.txt'}}
                note({'type':'tool_call','toolUse':tool})
                note({'type':'create_message','message':{'id':'tool-message','role':'assistant','content':[tool],'createdAt':1794186000000}})
                note({'type':'tool_result','messageId':'tool-result','toolUseId':'tool-example','content':'synthetic tool output','isError':False})
                finish(v['id'],p['text'])
            elif p['text']=='error':
                note({'type':'error','message':'Synthetic authentication failure','errorType':'Error','timestamp':'2026-11-09T01:00:00Z'})
                note({'type':'agent_turn_completed','reason':'model_authentication_failed','tokenUsage':{}})
            elif p['text'] in ('approval','question'):
                key='ask-'+v['id'];pending[key]=v
                if p['text']=='approval':
                    emit({'type':'request','id':key,'method':'droid.request_permission','params':{'toolUses':[{'toolUse':{'id':'tool-1','name':'Execute','input':{'command':'echo synthetic'}},'confirmationType':'exec','details':{'type':'exec','command':'echo synthetic'}}],'options':[{'label':'允许本次','value':'proceed_once'},{'label':'拒绝','value':'cancel'}]}})
                else:emit({'type':'request','id':key,'method':'droid.ask_user','params':{'toolCallId':'question-1','questions':[{'index':1,'topic':'Choice','question':'Choose one?','options':['Alpha','Beta']}]}})
            else:finish(v['id'],p['text'])
        elif method=='droid.interrupt_session':reply(v,{});note({'type':'agent_turn_completed','reason':'cancelled','tokenUsage':{}})
        else:emit({'id':v['id'],'error':{'code':-32601,'message':'Unknown method'}})


if __name__=='__main__':fake_cli()
