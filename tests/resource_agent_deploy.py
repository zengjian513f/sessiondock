#!/usr/bin/env python3
"""Offline contract for the opt-in system collector deployment handler."""
import sys
from pathlib import Path
from types import SimpleNamespace
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'deploy'))
from sdtargets import handler_for
from sdtargets.resource_agent import ResourceAgentHandler


def main():
    assert handler_for('resource-agent') is ResourceAgentHandler
    unit = (Path(__file__).resolve().parents[1]/'deploy/resource-agent.service').read_text()
    assert 'KillMode=control-group' in unit
    assert 'ProtectControlGroups=yes' in unit
    assert 'CPUQuota=100%' in unit
    assert '--memory-bandwidth on' in unit
    assert 'Wants=sys-fs-resctrl.mount' in unit
    assert 'CAP_DAC_OVERRIDE' in unit
    assert '-/sys/fs/resctrl' in unit
    assert '--io-events off' in unit
    assert 'Before=ssh' not in unit and 'RequiredBy=' not in unit
    handler = object.__new__(ResourceAgentHandler)
    handler.t = SimpleNamespace(prefix='/opt/resource-agent')
    handler.temp = '/tmp/private-stage'
    commands=[]
    handler.sh = SimpleNamespace(run=lambda cmd, **kw: commands.append(cmd) or (0,''))
    handler.swap();handler.restart()
    joined='\n'.join(commands)
    assert 'sudo -n install -m 0755' in joined
    assert 'resource-agent.new' in joined
    assert 'restart resource-agent.service' in joined
    assert 'restart ssh' not in joined and 'restart sessiondock' not in joined
    assert 'sys-fs-resctrl.mount' in joined
    assert 'cgroup.procs' not in joined
    handler.before=SimpleNamespace(active=True)
    handler.rollback('/opt/resource-agent/backup-deploy-test')
    assert 'umount' not in '\n'.join(commands)
    print('PASS system collector deploy: opt-in, root-owned binary, bounded service, no SSH/workload dependency')


if __name__ == '__main__':
    main()
