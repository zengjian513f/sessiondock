"""Synthetic native encrypted auth and local Factory billing endpoint."""
# run_validation: skip
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import Thread
import base64
import json
import os
import time

@contextmanager
def billing(fixture):
    from cryptography.hazmat.primitives.ciphers.aead import AESGCM
    secret='SYNTHETIC-DROID-ACCOUNT-TOKEN'
    key=os.urandom(32);iv=os.urandom(16)
    encrypted=AESGCM(key).encrypt(iv,json.dumps({'access_token':secret,'refresh_token':'NEVER-USE-THIS'}).encode(),None)
    encode=lambda b:base64.b64encode(b).decode()
    (fixture.config/'auth.v2.key').write_text(encode(key))
    (fixture.config/'auth.v2.file').write_text(':'.join(map(encode,[iv,encrypted[-16:],encrypted[:-16]])))
    state={'calls':0,'fail':False,'used':17}
    class Handler(BaseHTTPRequestHandler):
        def log_message(self,*args):pass
        def do_GET(self):
            assert self.path=='/api/billing/limits'
            assert self.headers['Authorization']=='Bearer '+secret
            state['calls']+=1
            self.send_response(503 if state['fail'] else 200)
            self.send_header('Content-Type','application/json');self.end_headers()
            limits={group:{window:{'usedPercent':state['used'] if group=='standard' else 0,'windowEnd':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime(time.time()+7200)) if group=='standard' else None} for window in ['fiveHour','weekly','monthly']} for group in ['standard','core']}
            self.wfile.write(json.dumps({'limits':limits,'private_fixture':secret}).encode())
    server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
    config=json.loads(fixture.launcher.read_text());profile=config['profiles'][0]
    profile['env']['FACTORY_API_BASE']=f'http://127.0.0.1:{server.server_port}'
    profile['env_remove'].append('FACTORY_API_KEY')
    fixture.launcher.write_text(json.dumps(config))
    thread=Thread(target=server.serve_forever,daemon=True);thread.start()
    try:yield state
    finally:server.shutdown();server.server_close();thread.join()
