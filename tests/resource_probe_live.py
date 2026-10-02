#!/usr/bin/env python3
"""Opt-in root/BPF integration: fixed lease, no renewal, stop and workload survival."""
# run_validation: skip
import argparse,json,os,socket,subprocess,tempfile,time
from pathlib import Path

def request(path,op,data=None):
 with socket.socket(socket.AF_UNIX) as s:
  s.settimeout(3);s.connect(str(path));s.sendall((json.dumps({'op':op,**({'data':data} if data is not None else {})})+'\n').encode())
  r=json.loads(s.makefile('rb').readline());assert r['ok'],r;return r['result']
def wait(fn,seconds=12):
 end=time.monotonic()+seconds
 while time.monotonic()<end:
  try:
   r=fn()
   if r:return r
  except (OSError,KeyError):pass
  time.sleep(.1)
 raise AssertionError('condition timed out')
def main():
 p=argparse.ArgumentParser();p.add_argument('--binary',type=Path,required=True);a=p.parse_args()
 assert os.geteuid()==0,'Explicit privileged live test only'
 with tempfile.TemporaryDirectory(prefix='resource-probe-live-') as tmp:
  root=Path(tmp);sock=root/'agent.sock';(root/'id').write_text('e'*32)
  log=open(root/'agent.log','w+')
  agent=subprocess.Popen([str(a.binary.resolve()),'--node-id-file',str(root/'id'),'--uid','0','--socket',str(sock),'--state',str(root/'state')],stdout=log,stderr=log)
  workload=subprocess.Popen(['sleep','150'])
  def diag():return request(sock,'resources')['diagnostic']
  try:
   wait(lambda: request(sock,'health'))
   assert diag()['state']=='off'
   start=time.monotonic();v=request(sock,'probe',{'enabled':True});assert v['remaining_seconds']<=60
   wait(lambda:diag()['state']=='active')
   print('active; testing 60s expiry with disconnected clients',flush=True)
   time.sleep(3)
   before=diag()['remaining_seconds'];after=request(sock,'probe',{'enabled':True})['remaining_seconds'];assert after<=before and after<60,(before,after)
   for _ in range(6):
    time.sleep(10)
    v=diag();print('elapsed',round(time.monotonic()-start,1),v,flush=True)
    if v['state']=='off':break
   wait(lambda:diag()['state']=='off',5)
   assert time.monotonic()-start<70
   values=request(sock,'resources')
   for key in ('nfs_read_bytes_per_second', 'disk_read_operations_per_second', 'disk_write_operations_per_second', 'nfs_read_operations_per_second', 'nfs_write_operations_per_second'):
    assert values['metric_availability'][key]['value'] is None
    assert all(s['metrics'][key]['value'] is None for s in values['samples'])
   assert values['metric_availability']['proc_storage_read_bytes_per_second']['status'] == 'partial'
   assert workload.poll() is None
   request(sock,'probe',{'enabled':True});wait(lambda:diag()['state']=='active')
   stopped=time.monotonic();request(sock,'probe',{'enabled':False});wait(lambda:diag()['state']=='off',5)
   assert workload.poll() is None
   print('PASS fixed60s/no-renewal/manual-stop/no-stale-values/workload-survival; stop seconds',time.monotonic()-stopped,flush=True)
  except BaseException:
   log.flush();log.seek(0);print(log.read()[-6000:]);raise
  finally:
   agent.terminate();agent.wait(timeout=15);workload.terminate();workload.wait(timeout=5);log.close()
if __name__=='__main__':main()
