#!/usr/bin/env python3
"""Private loopback node worker for the opt-in cross-host browser fixture.
Reads one fixture configuration then JSON commands on stdin; never discovers
production session roots. Launched by session_bundle_browser with --peer.
"""
# run_validation: skip
import base64
import json
import os
from pathlib import Path
import shutil
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
from urllib.request import build_opener, ProxyHandler


def main():
    config=json.loads(sys.stdin.readline());root=Path(config['root'])
    root.mkdir(parents=True,mode=0o700,exist_ok=False)
    try:
        serve(config,root)
    finally:
        shutil.rmtree(root)


def serve(config,root):
    native_baseline=None
    source=root/'source';destination=root/'destination'
    for path in config['roots'].values():Path(path).mkdir(parents=True,exist_ok=True)
    for cwd in config['cwds']:
        path=Path(cwd['path']);path.mkdir(parents=True,exist_ok=True);path.chmod(cwd['mode'])
    for folder in ('state','proc','ids','trash'):(destination/folder).mkdir(parents=True,exist_ok=True)
    for database,schema in config['schemas'].items():
        with sqlite3.connect(database) as db:db.executescript(schema)
    if config.get('native_codex'):
        from session_move_codex_real import AppServer, MODEL, EFFORT, digest, inventory, native_metadata
        home=Path(config['roots']['codex']);login=home/'auth.json'
        defaults=Path.home()/'.codex/config.toml';before=digest(defaults)
        login.symlink_to(Path.home()/'.codex/auth.json')
        try:
            with AppServer(str(Path.home()/'.local/bin/codex'),home,
                           Path(config['cwds'][0]['path']),root/'native-initialize.log') as server:
                thread=server.call('thread/start',{'model':MODEL,'config':{'model_reasoning_effort':EFFORT},
                    'cwd':config['cwds'][0]['path'],'historyMode':'paginated',
                    'approvalPolicy':'never','sandbox':'read-only'})['thread']
                server.turn(thread['id'],'TARGET_UNRELATED_KEEP')
            native_baseline={'sid':thread['id'],'files':inventory(home),'metadata':native_metadata(home,thread['id'])}
        finally:
            login.unlink(missing_ok=True)
            assert digest(defaults)==before,'daily configuration changed'
    (destination/'ids/node-id').write_text(config['node_id']+'\n')
    token=destination/'token';token.write_text(config['token']+'\n');token.chmod(0o600)
    def port():
        with socket.socket() as sock:sock.bind(('127.0.0.1',0));return sock.getsockname()[1]
    node_port,web_port=port(),port()
    env={k:v for k,v in os.environ.items() if not k.startswith('SESSIONDOCK_')}
    env.update({f'SESSIONDOCK_{k.upper()}_ROOT':v for k,v in config['roots'].items()})
    env.update(SESSIONDOCK_BIND=f'127.0.0.1:{web_port}',SESSIONDOCK_NODE_BIND=f'127.0.0.1:{node_port}',
        SESSIONDOCK_WEB_DIR=config['web'],SESSIONDOCK_STATE_DIR=str(destination/'state'),
        SESSIONDOCK_TRASH_DIR=str(destination/'trash'),
        SESSIONDOCK_PROC_ROOT=str(destination/'proc'),SESSIONDOCK_NODE_TOKEN_FILE=str(token),
        SESSIONDOCK_NODE_ID_FILE=str(destination/'ids/node-id'),SESSIONDOCK_NODE_PEERS='127.0.0.0/8')
    process=None;log=tempfile.TemporaryFile();opener=build_opener(ProxyHandler({}))
    def stop():
        nonlocal process
        if process and process.poll() is None:
            process.terminate()
            try:process.wait(timeout=10)
            except subprocess.TimeoutExpired:process.kill();process.wait(timeout=5)
        process=None
    def owned(path):
        path=Path(path)
        if not path.resolve().is_relative_to(root.resolve()):raise ValueError('outside private fixture')
        return path
    print(json.dumps({'node_port':node_port,'native_baseline':native_baseline}),flush=True)
    try:
        for line in sys.stdin:
            request=json.loads(line);op=request['operation'];result={}
            try:
                if op=='start':
                    assert process is None
                    process=subprocess.Popen([config['binary']],env=env,stdout=log,stderr=log)
                    for _ in range(200):
                        if process.poll() is not None:raise RuntimeError('private node exited')
                        try:
                            with opener.open(f'http://127.0.0.1:{web_port}/api/health',timeout=1) as response:assert response.status==200
                            break
                        except OSError:time.sleep(.05)
                    else:raise TimeoutError('private node startup')
                elif op=='stop':stop()
                elif op=='read':result['bytes']=base64.b64encode(owned(request['path']).read_bytes()).decode()
                elif op=='append':
                    with owned(request['path']).open('ab') as out:out.write(base64.b64decode(request['bytes']))
                elif op=='write':owned(request['path']).write_bytes(base64.b64decode(request['bytes']))
                elif op=='seed_cwd':
                    cwd=owned(request['path'])
                    for entry in request['entries']:
                        path=owned(cwd/entry['relative']);path.parent.mkdir(parents=True,exist_ok=True)
                        if entry['kind']=='directory':path.mkdir(exist_ok=True)
                        elif entry['kind']=='symlink':path.symlink_to(entry['target'])
                        else:path.write_bytes(base64.b64decode(entry['bytes']))
                        if entry['kind']!='symlink':path.chmod(entry['mode'])
                elif op=='receipt':
                    with sqlite3.connect(owned(request['path'])) as db:
                        result['receipt']=json.loads(db.execute('SELECT receipt FROM _sessiondock_clone_journal WHERE operation_id=?',(request['operation_id'],)).fetchone()[0])
                elif op=='finish':stop();print(json.dumps({'ok':True}),flush=True);break
                else:raise ValueError('unknown fixture command')
                print(json.dumps({'ok':True,**result}),flush=True)
            except Exception as error:print(json.dumps({'ok':False,'error':str(error)}),flush=True)
    finally:
        stop();log.close()


if __name__=='__main__':main()
