"""Opt-in Linux system collector. No dependency from workloads or SSH to this unit."""
from __future__ import annotations
import json
import shlex
import time
from pathlib import Path
from datetime import datetime, timezone
from .base import TargetHandler, ProbeResult, VerifyResult, register, failed_probe

q = shlex.quote
UNIT = 'resource-agent.service'
SOCKET = '/run/resource-agent/agent.sock'
ROOT = Path(__file__).resolve().parents[2]
HEALTH = """import socket,json
try:
 s=socket.socket(socket.AF_UNIX);s.settimeout(3);s.connect('/run/resource-agent/agent.sock');s.sendall(b'{"op":"health"}\\n');r=json.loads(s.makefile('rb').readline());print(json.dumps(r.get('result',{})))
except OSError: print('{}')
"""

@register
class ResourceAgentHandler(TargetHandler):
    kind = 'resource-agent'
    def probe(self):
        if self.t.prefix != '/opt/resource-agent': return ProbeResult(False, 'collector prefix must be /opt/resource-agent')
        code, out = self.sh.run('hostname\nid -u\nsudo -n true', timeout=20)
        if code:
            return failed_probe(self.t, code, out)
        lines = out.splitlines()
        if self.t.extra.get('expected_hostname') and lines[0] != self.t.extra['expected_hostname']:
            return ProbeResult(False, 'target identity mismatch')
        self.uid = int(lines[1])
        self.node_file = self.t.extra.get('node_id_file', '/srv/sessiondock/etc/node-id')
        code, out = self.sh.run('test -r '+q(self.node_file), timeout=10)
        if code: return ProbeResult(False, 'node identity file missing')
        code, active = self.sh.run('systemctl is-active '+UNIT, timeout=10)
        code, sha = self.sh.run('sha256sum '+q(self.t.prefix+'/bin/resource-agent')+' 2>/dev/null || true')
        self.before = ProbeResult(True, 'system collector', active=active.strip()=='active', binary_sha={'resource-agent':sha.split()[0] if sha.split() else ''})
        return self.before
    def plan(self):
        return ['install root-owned read-only collector and system unit', 'restart collector only; SSH and workloads have no dependency on it']
    def stage(self):
        _, temp = self.sh.run('mktemp -d /tmp/resource-agent-deploy.XXXXXX', check=True)
        self.temp = temp.strip()
        self.sh.upload(self.a.binaries['resource-agent'], self.temp+'/resource-agent')
        self.sh.upload(ROOT/'deploy/resource-agent.service', self.temp+'/resource-agent.service')
        self.sh.upload(ROOT/'deploy/sys-fs-resctrl.mount', self.temp+'/sys-fs-resctrl.mount')
        _, sha = self.sh.run('sha256sum '+q(self.temp+'/resource-agent'), check=True)
        if sha.split()[0] != self.a.sha256['resource-agent']: raise RuntimeError('collector upload hash mismatch')
        config = f'MONITOR_UID={self.uid}\nNODE_ID_FILE={self.node_file}\n'
        self.sh.run("printf '%s' "+q(config)+' > '+q(self.temp+'/resource-agent.env'),check=True)
    def backup(self):
        stamp = datetime.now(timezone.utc).strftime('%Y%m%d-%H%M%S')
        self.backup_dir = self.t.prefix+'/backup-deploy-'+self.a.short+'-'+stamp
        script = '\n'.join([
            'set -e', 'sudo -n mkdir -p '+q(self.backup_dir),
            'for f in '+q(self.t.prefix+'/bin/resource-agent')+' /etc/systemd/system/resource-agent.service /etc/systemd/system/sys-fs-resctrl.mount /etc/resource-agent.env; do',
            'if [ -f "$f" ]; then sudo -n cp -p "$f" '+q(self.backup_dir)+'/; fi', 'done',
            'systemctl is-enabled '+UNIT+' > '+q(self.temp+'/enabled')+' 2>/dev/null || true',
            'sudo -n cp '+q(self.temp+'/enabled')+' '+q(self.backup_dir+'/enabled'),
        ])
        self.sh.run(script,check=True)
        return self.backup_dir
    def swap(self):
        script = '\n'.join(['set -e',
            'sudo -n install -d -m 0755 '+q(self.t.prefix+'/bin'),
            'sudo -n install -m 0755 '+q(self.temp+'/resource-agent')+' '+q(self.t.prefix+'/bin/resource-agent.new'),
            'sudo -n mv '+q(self.t.prefix+'/bin/resource-agent.new')+' '+q(self.t.prefix+'/bin/resource-agent'),
            'sudo -n install -m 0644 '+q(self.temp+'/resource-agent.service')+' /etc/systemd/system/resource-agent.service',
            'sudo -n install -m 0644 '+q(self.temp+'/sys-fs-resctrl.mount')+' /etc/systemd/system/sys-fs-resctrl.mount',
            'sudo -n install -m 0600 '+q(self.temp+'/resource-agent.env')+' /etc/resource-agent.env',
            'sudo -n systemctl daemon-reload',
            'sudo -n systemctl enable '+UNIT,
        ])
        if self.t.prefix != '/opt/resource-agent': raise RuntimeError('collector prefix must match unit /opt/resource-agent')
        self.sh.run(script,check=True)
    def restart(self): self.sh.run('sudo -n systemctl restart '+UNIT,timeout=30,check=True)
    def verify(self):
        deadline = time.monotonic()+self.o.health_timeout
        data = {}
        while time.monotonic()<deadline:
            _, out = self.sh.run('python3 -c '+q(HEALTH), timeout=10)
            try: data = json.loads(out)
            except ValueError: data = {}
            if data.get('collector',{}).get('events') == 'bpf': break
            time.sleep(.5)
        _, sha = self.sh.run('sha256sum '+q(self.t.prefix+'/bin/resource-agent'))
        have = sha.split()[0] if sha.split() else ''
        _, active = self.sh.run('systemctl is-active '+UNIT)
        ok = (data.get('service')=='resource-agent' and data.get('collector',{}).get('events')=='bpf' and have==self.a.sha256['resource-agent'] and active.strip()=='active')
        detail = 'collector health/events/hash verified' if ok else 'collector health, BPF events, service state or hash verification failed'
        return VerifyResult(ok,detail,active=active.strip()=='active',binary_sha={'resource-agent':have})
    def rollback(self, backup_dir):
        if not backup_dir.startswith(self.t.prefix+'/backup-deploy-'): raise RuntimeError('invalid backup path')
        self.sh.run('sudo -n systemctl stop '+UNIT, timeout=20)
        _, has = self.sh.run('sudo -n test -f '+q(backup_dir+'/resource-agent')+' && echo yes')
        if has.strip() == 'yes':
            self.sh.run('sudo -n cp '+q(backup_dir+'/resource-agent')+' '+q(self.t.prefix+'/bin/resource-agent.new')+' && sudo -n mv '+q(self.t.prefix+'/bin/resource-agent.new')+' '+q(self.t.prefix+'/bin/resource-agent'),check=True)
            for name, target in [('sys-fs-resctrl.mount','/etc/systemd/system/sys-fs-resctrl.mount'),('resource-agent.service','/etc/systemd/system/resource-agent.service'),('resource-agent.env','/etc/resource-agent.env')]:
                self.sh.run('if sudo -n test -f '+q(backup_dir+'/'+name)+'; then sudo -n cp '+q(backup_dir+'/'+name)+' '+q(target)+'; else sudo -n rm -f '+q(target)+'; fi',check=True)
            self.sh.run('sudo -n systemctl daemon-reload',check=True)
            _, enabled = self.sh.run('sudo -n cat '+q(backup_dir+'/enabled'))
            self.sh.run('sudo -n systemctl '+('enable' if enabled.strip()=='enabled' else 'disable')+' '+UNIT,check=True)
            if self.before and self.before.active: self.restart()
        else:
            mount_backup = q(backup_dir+'/sys-fs-resctrl.mount')
            self.sh.run('if sudo -n test -f '+mount_backup+'; then sudo -n cp '+mount_backup+' /etc/systemd/system/sys-fs-resctrl.mount; else sudo -n rm -f /etc/systemd/system/sys-fs-resctrl.mount; fi',check=True)
            self.sh.run('sudo -n systemctl disable '+UNIT,check=True)
            self.sh.run('sudo -n rm -f /etc/systemd/system/resource-agent.service /etc/resource-agent.env '+q(self.t.prefix+'/bin/resource-agent'),check=True)
            self.sh.run('sudo -n systemctl daemon-reload',check=True)
    def write_marker(self):
        self.sh.run("printf '%s\\n' "+q(self.a.commit)+' | sudo -n tee '+q(self.t.prefix+'/deployed-commit')+' >/dev/null',check=True)
        self.sh.run('rm -rf '+q(self.temp),check=True)
    def prune_backups(self, keep): return []  # Keep explicit collector rollback records.
