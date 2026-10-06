#!/usr/bin/env python3
"""Free POSIX synthetic-identity host fixture for browser suites; fixed shell only."""
# run_validation: skip
from contextlib import contextmanager
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import time

from terminal_browser import REPO, SHELL_SCRIPT, stop


def wire(value):
    return json.dumps(value,separators=(",",":")).encode()+b"\n"


def line(stream):
    buffer=bytearray()
    while b"\n" not in buffer:
        part=stream.recv(65536)
        assert part,"host closed before the JSON acknowledgement"
        buffer.extend(part)
        assert len(buffer)<=1024*1024,"oversized host response"
    head,tail=buffer.split(b"\n",1)
    return json.loads(head),bytearray(tail)


def connect(record):
    stream=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM)
    stream.settimeout(3)
    stream.connect(record["sock"])
    return stream


def request(record,value):
    with connect(record) as stream:
        stream.sendall(wire(value))
        reply,tail=line(stream)
        assert not tail
        return reply


def guarded(instance,value,*,uid="codex:0123456789abcdef"):
    return {"op":"guarded_v1","expected_instance_id":instance,"expected_source":"codex",
            "expected_sid":"synthetic-native-sid","expected_uid":uid,"request":value}


@contextmanager
def host(root,instance,*,uid="codex:0123456789abcdef",tty_file=None,name="synthetic-identity-host"):
    environment={key:value for key,value in os.environ.items() if key in {"PATH","LANG","LC_ALL","LC_CTYPE"}}
    environment["TERM"]="xterm-256color"
    script=SHELL_SCRIPT
    if tty_file is not None:
        tty_file=Path(tty_file)
        assert tty_file.resolve().is_relative_to(root.resolve())
        environment["SESSIONDOCK_TEST_TTY_FILE"]=str(tty_file)
        script='tty > "$SESSIONDOCK_TEST_TTY_FILE"\n'+script
    metadata={"source":"codex","sid":"synthetic-native-sid","uid":uid,"instance_id":instance}
    process=subprocess.Popen([str(REPO/"target/debug/ptyhost"),"--dir",str(root/"host"),"run","--name",name,
        "--cwd",str(root/"work"),"--cols","80","--rows","24","--meta",json.dumps(metadata),"--",shutil.which("sh"),"-c",script],
        cwd=root/"work",env=environment,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    record=None
    try:
        deadline=time.monotonic()+5
        path=root/"host"/(name+".json")
        while time.monotonic()<deadline:
            assert process.poll() is None,"synthetic host exited early"
            if path.is_file():
                record=json.loads(path.read_text())
                if record["host_pid"]==process.pid:
                    try:
                        if "RS_SHELL_READY" in request(record,{"op":"capture","styled":False}).get("text",""):
                            break
                    except (OSError,json.JSONDecodeError):
                        pass
            time.sleep(.03)
        else:
            raise AssertionError("isolated shell startup timeout")
        yield process,record
    finally:
        if record and process.poll() is None:
            try:
                request(record,guarded(instance,{"op":"send","text":"quit\r"},uid=uid))
                process.wait(timeout=4)
            except (OSError,subprocess.TimeoutExpired):
                pass
        stop(process)  # Only the exact test child, never a name or guessed PID.
