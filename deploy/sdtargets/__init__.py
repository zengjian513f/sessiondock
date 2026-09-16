"""SessionDock fleet deployment: target handlers.

`base` holds the shared contract (Target, Artifacts, Shell, TargetHandler).
One module per target kind registers itself in `HANDLERS`:

    linux.py    -> "linux-node"   systemd --user unit, build-machine glibc binaries
    hub.py      -> "hub"          sessiondock-hub on the central VPS
    macos.py    -> "macos-node"   launchd LaunchAgent, built natively on the node
    windows.py  -> "windows-node" pythonw supervisor in desktop session 1, built natively
"""
from .base import (Artifacts, DeployOptions, HANDLERS, ProbeResult, Shell,
                   Target, TargetHandler, VerifyResult, handler_for, load_targets)

__all__ = ["Artifacts", "DeployOptions", "HANDLERS", "ProbeResult", "Shell",
           "Target", "TargetHandler", "VerifyResult", "handler_for", "load_targets"]
