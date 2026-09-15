"""Hub handler (kind "hub"): `sessiondock-hub` on the central VPS.

Same shape as a Linux node (`<prefix>/{bin,web,etc,hub,audit}`, systemd --user unit,
build-machine glibc binary), except that the hub has no session hosts: there is no
`host/` directory, `ptyhost` is never shipped, and the probe's pid list is only a
sanity check that nothing named ptyhost runs beside the hub.
"""
from __future__ import annotations

from .base import register
from .linux import LinuxNodeHandler


@register
class HubHandler(LinuxNodeHandler):
    kind = "hub"
    default_unit = "sessiondock-hub.service"
    ships_ptyhost = False

    def restart_note(self, before) -> str:
        return "no session hosts on the hub"
