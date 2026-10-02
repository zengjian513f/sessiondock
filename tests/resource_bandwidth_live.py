#!/usr/bin/env python3
# run_validation: skip
"""Explicit root-only live MBM test, private agent/workers and monitor groups.
Pass a local benchmark worker binary; never uses production session state.
"""
import argparse,json,os,pathlib,socket,subprocess,tempfile,time

def request(path,op,data=None):
 with socket.socket(socket.AF_UNIX) as s:
  s.settimeout(3);s.connect(str(path));s.sendall((json.dumps({'op':op,**({'data':data} if data is not None else {})})+'\n').encode())
  r=json.loads(s.makefile().readline());assert r['ok'],r;return r['result']
def wait(fn,seconds=20):
 end=time.monotonic()+seconds
 while time.monotonic()<end:
  try:
   value=fn()
   if value:return value
  except (OSError,KeyError):pass
  time.sleep(.2)
 raise AssertionError('condition timeout')
def main():
 ap=argparse.ArgumentParser();ap.add_argument('--agent',required=True);ap.add_argument('--worker',required=True);ap.add_argument('--groups',type=int,default=64);a=ap.parse_args()
 assert os.geteuid()==0
 root=pathlib.Path('/sys/fs/resctrl');mounted=not os.path.ismount(root);workers=[];unit='resource-agent-mbm-validation';prefix='sessiondock-'+('e'*32)+'-';rows=[]
 if mounted:subprocess.run(['mount','-t','resctrl','resctrl',str(root)],check=True)
 others={p.name for p in (root/'mon_groups').iterdir()}
 with tempfile.TemporaryDirectory(prefix='mbm-live-') as temp:
  tmp=pathlib.Path(temp);sock=tmp/'agent.sock';(tmp/'node').write_text('e'*32)
  try:
   owners=[];sessions=[]
   for i in range(a.groups):
    w=subprocess.Popen([a.worker,str(32+i%64)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True);workers.append(w);assert json.loads(w.stdout.readline())['ready']==w.pid
    process={'pid':w.pid,'start':int(pathlib.Path(f'/proc/{w.pid}/stat').read_text().rsplit(')',1)[1].split()[19])}
    session={'node_id':'e'*32,'source':'codex','sid':f'fixture-{i}'};owners.append({'process':process,'session':session});sessions.append(session)
   # Exercise the same filesystem/capability restrictions as the production unit.
   cmd=['systemd-run','--unit='+unit,'--collect','--property=ProtectSystem=strict','--property=ProtectHome=read-only','--property=ProtectKernelTunables=yes','--property=NoNewPrivileges=yes','--property=CapabilityBoundingSet=CAP_BPF CAP_PERFMON CAP_SYS_RESOURCE CAP_SYS_PTRACE CAP_DAC_READ_SEARCH CAP_DAC_OVERRIDE CAP_CHOWN', '--property=ReadWritePaths='+str(tmp)+' /sys/fs/resctrl /run/resource-agent',os.path.abspath(a.agent),'--node-id-file',str(tmp/'node'),'--uid','0','--socket',str(sock),'--state',str(tmp/'state'),'--memory-bandwidth','on','--events','off']
   subprocess.run(cmd,check=True,stdout=subprocess.DEVNULL)
   wait(lambda:request(sock,'health'))
   catalog={'node_id':'e'*32,'boot_id':pathlib.Path('/proc/sys/kernel/random/boot_id').read_text().strip(),'owners':owners,'sessions':sessions}
   request(sock,'catalog',catalog)
   wait(lambda:len(request(sock,'resources')['session_measurements'])==a.groups,20)
   for w in workers:w.stdin.write('copy 640\n');w.stdin.flush()
   start=time.monotonic()
   for _ in range(4):
    time.sleep(5)
    before=time.monotonic();r=request(sock,'resources');ms=(time.monotonic()-before)*1000
    values=[s['metrics']['memory_bandwidth_bytes_per_second']['value'] for s in r['sessions']]
    row={'elapsed':time.monotonic()-start,'api_ms':ms,'sessions':len(values),'known':sum(v is not None for v in values),'positive':sum(v is not None and v>0 for v in values),'sum_bytes_per_second':sum(v or 0 for v in values),'example':r['session_measurements'][:1]};rows.append(row);print(json.dumps(row),flush=True)
   assert rows[-1]['sessions']==a.groups and rows[-1]['positive']==a.groups,rows
   assert r['diagnostic']['state'] in ('off','unsupported')
   assert all('memory_bandwidth_bytes_per_second' not in p['metrics'] for p in r['samples'])
   subprocess.run(['systemctl','stop',unit],check=True)
   assert all(w.poll() is None for w in workers),'collector stopped a workload'
   assert not any(p.name.startswith(prefix) for p in (root/'mon_groups').iterdir())
   print('PASS live groups, positive rates, cached API, independent of I/O probe, no per-process duplication, cleanup and workload survival',flush=True)
  except BaseException:
   subprocess.run(['journalctl','-u',unit,'-n','20','--no-pager']);raise
  finally:
   subprocess.run(['systemctl','stop',unit],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
   for w in workers:
    if w.poll() is None:w.terminate()
    w.wait(timeout=5)
   # A failed/killed test must not leave its own monitoring groups behind.
   for p in (root/'mon_groups').iterdir():
    if p.name.startswith(prefix):p.rmdir()
   assert {p.name for p in (root/'mon_groups').iterdir()}==others
   if mounted and not others:subprocess.run(['umount',str(root)],check=True)
if __name__=='__main__':main()
