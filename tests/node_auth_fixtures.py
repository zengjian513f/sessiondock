"""Shared fixture helpers extracted from the former `node_auth_suite.py` (removed 2026-10-06
with the non-browser suites); imported by browser suites."""
# run_validation: skip
from __future__ import annotations
import socket
from pathlib import Path


TOKEN = "suite-t0ken.suite-t0ken.suite-t0ken.suite-t0ken~"


GOOD = {"X-SessionDock-Protocol": "1", "X-SessionDock-Node-Token": TOKEN}


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def node_env(root: Path, node_port: int, peers: str):
    token = root / "node-token"
    if not token.exists():
        token.touch(mode=0o600)
        token.write_text(TOKEN + "\n", encoding="utf-8")
    return {
        "SESSIONDOCK_NODE_BIND": f"127.0.0.1:{node_port}",
        "SESSIONDOCK_NODE_TOKEN_FILE": str(token),
        "SESSIONDOCK_NODE_ID_FILE": str(root / "ids" / "node-id"),
        "SESSIONDOCK_NODE_PEERS": peers,
    }
