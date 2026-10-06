#!/usr/bin/env python3
"""Durable SSH launch evidence through real agents, Hub and Chromium.

Default: private synthetic process snapshots and saved event records.
--kernel: real BPF + short OpenSSH launches against a private loopback sshd;
requires root, a non-root --ssh-user, sshd and Chromium. No paid CLI or production
runtime/session directories are used. Process snapshots expose only fixture PIDs.
"""
import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import pwd
import shlex
import socket
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace

from playwright.sync_api import sync_playwright
from history_fixtures import BINARY, Corpus, codex_row, isolated_server, get_json
from hub_fixtures import Hub, scoped
from node_auth_fixtures import TOKEN, free_port, node_env
from process_links_browser import wait_for
from spawned_by_fixtures import proc_pid, START


def request(path, op, data=None):
    with socket.socket(socket.AF_UNIX) as stream:
        stream.settimeout(5)
        stream.connect(str(path))
        stream.sendall(json.dumps({'op':op, **({'data':data} if data is not None else {})}).encode()+b'\n')
        response = json.loads(stream.makefile('rb').readline())
        assert response['ok'], response
        return response['result']


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
            raise AssertionError('private fixture process did not exit')


class Fixture:
    def __init__(self, root, binary, stack, uids, kernel):
        self.root, self.binary, self.stack, self.kernel = root, binary.resolve(), stack, kernel
        self.corpora, self.procs, self.agents, self.nodes, self.local = [], [], [], [], []
        self.uids = uids
        for index, name in enumerate(('origin', 'execution')):
            corpus = Corpus(root/name)
            for sid in ('parent', 'other'):
                corpus.put(sid, 'codex', [codex_row('session_meta', {'id':sid, 'cwd':'/synthetic/ssh-events',
                    'timestamp':'2026-09-11T10:00:00Z'}),
                    codex_row('response_item', {'type':'message','role':'user','content':name+' '+sid})], [])
            (corpus.root/'ids').mkdir()
            nid = ('a' if index == 0 else 'b')*32
            (corpus.root/'ids/node-id').write_text(nid)
            proc = corpus.root/'proc'
            (proc/'sys/kernel/random').mkdir(parents=True)
            (proc/'sys/kernel/random/boot_id').write_text(Path('/proc/sys/kernel/random/boot_id').read_text() if kernel else 'event-boot')
            (proc/'stat').write_text(Path('/proc/stat').read_text() if kernel else 'btime 1700000000\n')
            (proc/'net').mkdir()
            self.corpora.append(corpus)
            self.procs.append(proc)
            self.nodes.append(SimpleNamespace(name=name, nid=nid, port=free_port(), token=TOKEN))

    def agent(self, index):
        corpus = self.corpora[index]
        log = self.stack.enter_context((self.root/f'agent-{index}.log').open('ab'))
        process = subprocess.Popen([str(self.binary.with_name('resource-agent')), '--uid',str(self.uids[index]),
            '--node-id-file',str(corpus.root/'ids/node-id'), '--socket',str(corpus.root/'agent.sock'),
            '--state',str(corpus.root/'agent-state.json'), '--proc-root',str(self.procs[index]),
            '--events','auto' if self.kernel else 'off'], stdout=log, stderr=log)
        self.stack.callback(stop, process)
        wait_for(lambda: (corpus.root/'agent.sock').exists() or process.poll() is not None)
        assert process.poll() is None, (self.root/f'agent-{index}.log').read_text()
        if self.kernel:
            wait_for(lambda: request(corpus.root/'agent.sock','health')['collector']['events'] != 'starting')
            health = request(corpus.root/'agent.sock','health')
            assert health['collector']['events'] == 'bpf' and health['collector']['lost_events'] == 0 and health['collector']['ssh_connections'], (health,(self.root/f'agent-{index}.log').read_text())
        return process

    def start(self):
        for index, corpus in enumerate(self.corpora):
            self.agents.append(self.agent(index))
            env = node_env(corpus.root,self.nodes[index].port,'127.0.0.0/8')
            env.update(SESSIONDOCK_PROC_ROOT=str(self.procs[index]),
                       SESSIONDOCK_RESOURCE_AGENT_SOCKET=str(corpus.root/'agent.sock'))
            self.local.append(self.stack.enter_context(isolated_server(corpus,self.binary,extra_env=env)))
        (self.root/'hub').mkdir()
        self.hub = Hub(self.binary.with_name('sessiondock-hub'),self.root/'hub',self.nodes)
        self.stack.callback(self.hub.stop)

    def restart_agents(self):
        for index, process in enumerate(self.agents):
            stop(process)
            self.agents[index] = self.agent(index)

    def report(self,index):
        return request(self.corpora[index].root/'agent.sock','report')

    def bindings(self):
        return self.report(1)['bindings']

    def browser(self,pw):
        self.hub.start()
        launch = {'headless':True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**launch)
        self.stack.callback(browser.close)
        context = browser.new_context(service_workers='block')
        page = context.new_page()
        page.goto(f'http://127.0.0.1:{self.hub.port}',wait_until='networkidle')
        uid = scoped(self.nodes[0].nid,self.corpora[0].uid('parent'))
        item = page.locator(f'#side .item[data-uid="{uid}"]')
        item.click()
        toggle = page.get_by_role('button',name='列表资源',exact=True)
        if toggle.get_attribute('aria-pressed') != 'true': toggle.click()
        return page,item


def synthetic(fixture,pw):
    now = time.time()
    owner = {'node_id':fixture.nodes[0].nid,'source':'codex','sid':'parent'}
    other = {**owner,'sid':'other'}
    conn = lambda port: {'client_ip':'::1','client_port':port,'server_ip':'::1','server_port':50022}
    proc = lambda pid: {'pid':pid,'start':pid*100}
    def record(pid,port,start,end,session,shared=False):
        return {'process':proc(pid),'connection':conn(port),'opened_at':now+start,'closed_at':now+end,
                'ssh':True,'shared':shared,'session':session,'launch_chain':[]}
    records = [record(200,40000,-60,-55,owner),record(201,40000,-30,-25,other),
               record(202,40001,-30,-25,owner,True),record(203,40002,-30,-25,owner),
               record(204,40002,-29,-24,other),record(205,40003,-60,-55,owner),
               record(206,40003,-30,-25,None)]
    for index in range(2):
        saved = {'node_id':fixture.nodes[index].nid,'boot_id':'event-boot',
                 'catalog':{'node_id':fixture.nodes[index].nid,'boot_id':'event-boot','owners':[],'sessions':[]},'links':[],'bindings':[],
                 'connections':{'records':records if index==0 else [],'parents':[], 'origins':[], 'shared':[]}}
        if index==1:
            # Origin exec and intermediate parent exited before the first sample;
            # all live grandchildren retain the original connection timestamp.
            for pid,port,at in [(500,40000,-59),(501,40000,-29),(502,40001,-29),
                                (503,40002,-28),(504,40003,-29),(505,40000,-10)]:
                START[pid] = pid*100
                proc_pid(fixture.procs[1],pid,'python',['python','background.py'],1)
                saved['connections']['parents'].extend([[proc(pid),proc(pid+1000)],[proc(pid+1000),proc(pid+2000)]])
                saved['connections']['origins'].append([proc(pid+2000),{'connection':conn(port),'at':now+at}])
        else:
            proc_pid(fixture.procs[0],100,'codex',['codex'],1,fds={3:str(fixture.corpora[0].paths['parent'])})
        for entry in fixture.procs[index].iterdir():
            if entry.name.isdigit():
                stat=entry/'stat'
                stat.write_text(stat.read_text().rstrip()+' 0 0\n')
                (entry/'smaps_rollup').write_text('Pss: 16 kB\n')
        (fixture.corpora[index].root/'agent-state.json').write_text(json.dumps(saved))
    fixture.start()
    page,item = fixture.browser(pw)
    wait_for(lambda: {b['process']['pid'] for b in fixture.bindings()} == {500,501})
    bindings = {b['process']['pid']:b for b in fixture.bindings()}
    assert bindings[500]['session']['sid']=='parent' and bindings[501]['session']['sid']=='other', bindings
    wait_for(lambda: item.locator('[data-resource="process_count"] .item-resource-value').inner_text()=='2')
    item.locator('.item-resources').click()
    page.wait_for_function("document.querySelectorAll('.session-resources .sr-node').length===2")
    page.get_by_role('button',name='关闭资源面板').click()
    fixture.hub.stop()
    fixture.restart_agents()
    fixture.hub.start()
    page.reload(wait_until='networkidle')
    wait_for(lambda: {b['process']['pid'] for b in fixture.bindings()} == {500,501})
    # PID reuse cannot inherit the old process's origin or durable binding.
    stat=fixture.procs[1]/'500/stat'
    stat.write_text(stat.read_text().replace(' 50000 ',' 50001 '))
    wait_for(lambda: not any(b['process']['pid']==500 for b in fixture.bindings()))
    page.evaluate('SessionDockSidebarResources.refresh()')
    wait_for(lambda: item.locator('[data-resource="process_count"] .item-resource-value').inner_text()=='1')
    print('PASS closed IPv6 SSH evidence, late descendants, collector/Hub restart, tuple reuse, unknown/ambiguous/shared rejection, PID reuse and browser resources',flush=True)


def kernel(fixture,pw,user):
    root=fixture.root
    root.chmod(0o711)  # The isolated sshd's user can traverse only its own directory.
    remote=root/'remote'
    remote.mkdir(mode=0o700)
    account=pwd.getpwnam(user)
    os.chown(remote,account.pw_uid,account.pw_gid)
    for key in ('host','client'):
        subprocess.run(['ssh-keygen','-q','-t','ed25519','-N','','-f',str(root/key)],check=True,timeout=10)
    authorized=remote/'authorized_keys'
    authorized.write_text((root/'client.pub').read_text())
    os.chown(authorized,account.pw_uid,account.pw_gid)
    authorized.chmod(0o600)
    port=free_port()
    config=root/'sshd.conf'
    config.write_text(f'''Port {port}
ListenAddress 127.0.0.1
ListenAddress ::1
HostKey {root/'host'}
PidFile {root/'sshd.pid'}
AuthorizedKeysFile {authorized}
StrictModes no
UsePAM no
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin no
AllowUsers {user}
PrintMotd no
PrintLastLog no
LogLevel ERROR
''')
    sshlog=fixture.stack.enter_context((root/'sshd.log').open('ab'))
    server=subprocess.Popen(['/usr/sbin/sshd','-D','-e','-f',str(config)],stdout=sshlog,stderr=sshlog)
    fixture.stack.callback(stop,server)
    def listening():
        try:
            with socket.create_connection(('127.0.0.1',port),timeout=.2): return True
        except OSError: return False
    wait_for(listening)
    remote_script=remote/'worker.py'
    remote_script.write_text('''import os,sys,time
from pathlib import Path
root=Path(sys.argv[1]); tag=sys.argv[2]
if os.fork(): os._exit(0)
os.setsid()
fd=os.open('/dev/null',os.O_RDWR)
for target in (0,1,2): os.dup2(fd,target)
if fd>2: os.close(fd)
time.sleep(3.5)
if os.fork(): os._exit(0)
(root/(tag+'.pid')).write_text(str(os.getpid()))
while not (root/'stop').exists(): time.sleep(.05)
''')
    os.chown(remote_script,account.pw_uid,account.pw_gid)
    # Root launches SSH; the other UID executes the command. Two private /proc
    # views expose only fixture processes, so none of the host's real sessions
    # or heavyweight process resource scans participate in this browser suite.
    launcher_script=root/'launcher.py'
    launcher_script.write_text('''import subprocess,sys,json,time
for line in sys.stdin:
    command=json.loads(line); started=time.monotonic()
    result=subprocess.run(command,capture_output=True,text=True,timeout=15)
    print(json.dumps({'code':result.returncode,'elapsed':time.monotonic()-started,'out':result.stdout,'err':result.stderr}),flush=True)
''')
    env=dict(os.environ,CODEX_THREAD_ID='parent')
    launcher=subprocess.Popen([sys.executable,'-u',str(launcher_script)],env=env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
    fixture.stack.callback(stop,launcher)
    (fixture.procs[0]/str(launcher.pid)).symlink_to(Path('/proc')/str(launcher.pid))
    # Cleanup waits for the exact workers started by this fixture before the
    # temporary home disappears; no name-based process termination.
    workers=[]
    def cleanup():
        (remote/'stop').touch()
        for pid in workers:
            wait_for(lambda p=pid: not Path(f'/proc/{p}/stat').exists()
                     or Path(f'/proc/{p}/stat').read_text().rsplit(')',1)[1].split()[0]=='Z')
    fixture.stack.callback(cleanup)
    fixture.start()
    wait_for(lambda: any(b['process']['pid']==launcher.pid and b['session']['sid']=='parent' for b in fixture.report(0)['bindings']))
    common=['ssh','-F','/dev/null','-p',str(port),'-i',str(root/'client'),'-o','IdentitiesOnly=yes',
            '-o','BatchMode=yes','-o','StrictHostKeyChecking=no','-o','UserKnownHostsFile=/dev/null','-o','LogLevel=ERROR']
    def run(command):
        launcher.stdin.write(json.dumps(command)+'\n'); launcher.stdin.flush()
        value=json.loads(launcher.stdout.readline())
        assert value['code']==0,value
        return value
    def launch(tag,options,host):
        command=shlex.join(['/usr/bin/python3',str(remote_script),str(remote),tag])
        value=run([*common,*options,f'{user}@{host}',command])
        # Client returns before the delayed grandchild is even created.
        assert value['elapsed']<3.0,value
        wait_for(lambda:(remote/(tag+'.pid')).exists())
        pid=int((remote/(tag+'.pid')).read_text()); workers.append(pid)
        exposed=fixture.procs[1]/str(pid)
        exposed.symlink_to(Path('/proc')/str(pid))
        os.lchown(exposed,account.pw_uid,account.pw_gid)
        return pid
    first=launch('short-v4',['-4','-o','ControlMaster=no'],'127.0.0.1')
    second=launch('short-v6',['-6','-o','ControlMaster=no'],'::1')
    wait_for(lambda:len([r for r in fixture.report(0).get('connections',[]) if (r.get('session') or {}).get('sid')=='parent' and r['closed_at'] and not r['shared']])>=2)
    assert not fixture.report(0)['outgoing'], 'SSH clients must never appear in a polling snapshot'
    incoming=wait_for(lambda: {r['process']['pid']:r for r in fixture.report(1)['incoming']} if
                      {first,second} <= {r['process']['pid'] for r in fixture.report(1)['incoming']} else None)
    assert all(incoming[pid]['started_at']>incoming[pid]['connection_at']+3 for pid in (first,second)),incoming
    fixture.restart_agents()  # No Hub correlation has happened yet.
    page,item=fixture.browser(pw)
    wait_for(lambda:{first,second} <= {b['process']['pid'] for b in fixture.bindings()})
    page.evaluate('SessionDockSidebarResources.refresh()')
    wait_for(lambda:item.locator('[data-resource="process_count"] .item-resource-value').inner_text()=='3')
    item.locator('.item-resources').click()
    page.wait_for_function("document.querySelectorAll('.session-resources .sr-node').length===2")
    page.get_by_role('button',name='关闭资源面板').click()
    print('PASS real BPF IPv4/IPv6 short SSH, delayed orphan grandchildren, no live-client snapshot, saved evidence before Hub startup and browser resource ownership',flush=True)
    control=root/'control'
    run([*common,'-M','-S',str(control),'-f','-N',f'{user}@127.0.0.1'])
    fixture.stack.callback(lambda: subprocess.run([*common,'-S',str(control),'-O','exit',f'{user}@127.0.0.1'],capture_output=True,timeout=10))
    shared=launch('shared',['-S',str(control)],'127.0.0.1')
    sample=fixture.report(1)['sampled_at']
    wait_for(lambda:fixture.report(1)['sampled_at']>sample+4)
    assert not any(b['process']['pid']==shared for b in fixture.bindings())
    assert any(r['shared'] for r in fixture.report(0)['connections']), fixture.report(0)['connections']
    page.evaluate('SessionDockSidebarResources.refresh()')
    assert item.locator('[data-resource="process_count"] .item-resource-value').inner_text()=='3'
    for index in (0,1):
        assert fixture.report(index)['collector']['lost_events']==0,fixture.report(index)['collector']
    print('PASS real ControlMaster/ControlPersist fork/listener exclusion; no attribution by shared transport',flush=True)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--kernel',action='store_true')
    parser.add_argument('--ssh-user')
    args=parser.parse_args()
    if args.kernel:
        assert os.geteuid()==0 and args.ssh_user, '--kernel requires root and --ssh-user'
        uid=pwd.getpwnam(args.ssh_user).pw_uid
        assert uid!=0
    else: uid=os.getuid()
    with tempfile.TemporaryDirectory(prefix='ssh-events-browser-') as temporary, sync_playwright() as pw, ExitStack() as stack:
        fixture=Fixture(Path(temporary),args.binary,stack,[os.getuid(),uid],args.kernel)
        try:
            if args.kernel: kernel(fixture,pw,args.ssh_user)
            else: synthetic(fixture,pw)
        except Exception:
            for index, corpus in enumerate(fixture.corpora):
                try:
                    print('REPORT', index, json.dumps(fixture.report(index)), file=sys.stderr)
                    saved=corpus.root/'agent-state.json'
                    if saved.exists(): print('EVIDENCE',index,saved.read_text()[-20000:],file=sys.stderr)
                except Exception: pass
            for path in Path(temporary).glob('*.log'):
                print(path.name,path.read_text()[-12000:],file=sys.stderr)
            raise


if __name__=='__main__': main()
