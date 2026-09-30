#!/usr/bin/env python3
"""Independent collector lifetime, persistence and unavailable-node behavior.
Run alongside process_links_browser.py --with-agent for the user-facing path.
"""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
from history_parity import BINARY
from spawned_by_suite import proc_pid


def request(path, op, data=None):
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(3)
        client.connect(str(path))
        client.sendall((json.dumps({'op':op, **({'data':data} if data is not None else {})})+'\n').encode())
        response = json.loads(client.makefile('rb').readline())
        assert response['ok'], response
        return response['result']


def wait(function):
    until=time.monotonic()+15
    while time.monotonic()<until:
        try:
            result=function()
            if result:return result
        except (OSError, ValueError):pass
        time.sleep(.1)
    raise AssertionError('collector condition timed out')


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='resource-agent-test-') as tmp:
        root=Path(tmp);proc=root/'proc';sock=root/'agent.sock'
        (proc/'sys/kernel/random').mkdir(parents=True)
        (proc/'sys/kernel/random/boot_id').write_text('boot-test')
        (proc/'stat').write_text('btime 1700000000\n')
        proc_pid(proc,100,'python',['python','worker'],1)
        (root/'node-id').write_text('a'*32)
        argv=[str(args.binary.resolve().with_name('resource-agent')),'--node-id-file',str(root/'node-id'),'--uid',str(os.getuid()),'--socket',str(sock),'--state',str(root/'state.json'),'--proc-root',str(proc),'--events','off']
        # A workload does not belong to the collector's process group/cgroup.
        workload=subprocess.Popen(['sleep','60'])
        agent=None
        try:
            for restarted in (False,True):
                agent=subprocess.Popen(argv,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,text=True)
                wait(lambda: request(sock,'health'))
                assert sock.stat().st_mode & 0o777 == 0o600
                if not restarted:
                    session={'node_id':'a'*32,'source':'codex','sid':'test','created':1}
                    catalog={'node_id':'a'*32,'boot_id':'boot-test','sessions':[session],'owners':[{'process':{'pid':100,'start':10000},'session':session}]}
                    report=request(sock,'catalog',catalog)
                    assert report['bindings'][0]['session']['sid']=='test'
                    duplicate=subprocess.run(argv,capture_output=True,text=True,timeout=5)
                    assert duplicate.returncode!=0 and 'already in use' in duplicate.stderr
                    assert request(sock,'report')['bindings']
                    data=request(sock,'resources')
                    assert data['availability']=='observed'
                    assert 'nfs_per_session' in data['unavailable']
                    assert 'gpu' in data['unavailable']
                else:
                    assert request(sock,'report')['bindings'][0]['session']['sid']=='test'
                    path=proc/'100/stat'
                    path.write_text(path.read_text().replace('10000','10001'))
                    wait(lambda: not request(sock,'report')['bindings'])
                agent.terminate();agent.wait(timeout=10);agent=None
                assert workload.poll() is None, 'stopping collector must not stop workload'
            print('PASS independent service: persistence, PID reuse, duplicate writer, unavailable metrics, workload survives stop')
        finally:
            if agent and agent.poll() is None:agent.terminate();agent.wait(timeout=10)
            workload.terminate();workload.wait(timeout=10)


if __name__ == '__main__':
    main()
