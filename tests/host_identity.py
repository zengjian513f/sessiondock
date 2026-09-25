#!/usr/bin/env python3
"""Free POSIX host-instance guard check; synthetic metadata and fixed shell only."""
from contextlib import contextmanager
import json
import os
from pathlib import Path
import shutil
import socket
import struct
import subprocess
import tempfile
import time
import uuid

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


def read_until(stream,buffer,wanted):
    text=bytearray()
    deadline=time.monotonic()+5
    while time.monotonic()<deadline:
        while len(buffer)>=5:
            kind,length=buffer[0],int.from_bytes(buffer[1:5],"big")
            assert length<=8*1024*1024
            if len(buffer)<5+length: break
            payload=buffer[5:5+length];del buffer[:5+length]
            if kind==1: text.extend(payload)
            if wanted in text:return
        part=stream.recv(65536)
        assert part,"host closed before expected output"
        buffer.extend(part)
    raise AssertionError("guarded attach output timeout")


def main():
    if os.name!="posix" or not shutil.which("sh"):
        raise SystemExit("This isolated real-shell test needs POSIX sh; portable fake-peer tests are separate.")
    old="synthetic-"+uuid.uuid4().hex
    new="synthetic-"+uuid.uuid4().hex
    with tempfile.TemporaryDirectory(prefix="sessiondock-identity-") as temporary:
        root=Path(temporary)
        (root/"host").mkdir(mode=0o700);(root/"work").mkdir()
        with host(root,old) as (process,record):
            before=request(record,{"op":"info"})
            assert before["capabilities"]["instance_guard"]==1
            assert "instance_guard" not in before
            rejected=request(record,guarded(new,{"op":"attach","cols":100,"rows":50,"replay":False}))
            assert rejected["ok"] is False
            rejected=request(record,guarded(new,{"op":"send","text":"quit\r"}))
            assert rejected["ok"] is False
            after=request(record,{"op":"info"})
            assert after["exited"] is False and after["info"]["cols"]==80 and after["info"]["rows"]==24
            with connect(record) as stream:
                stream.sendall(wire(guarded(old,{"op":"attach","cols":100,"rows":40,"replay":True})))
                ack,buffer=line(stream)
                assert ack["instance_guard"]=={"version":1,"instance_id":old}
                assert (ack["cols"],ack["rows"])==(100,40)
                read_until(stream,buffer,b"RS_SHELL_READY")
                payload=b"next\r";stream.sendall(bytes([1])+struct.pack("!I",len(payload))+payload)
                read_until(stream,buffer,b"RS_AFTER_RESTART")
            first_pid=record["pid"]
        with host(root,new) as (process,record):
            assert record["pid"]!=first_pid
            rejected=request(record,guarded(old,{"op":"send","text":"quit\r"}))
            assert rejected["ok"] is False and process.poll() is None
            response=request(record,guarded(new,{"op":"info"}))
            assert response["instance_guard"]=={"version":1,"instance_id":new}
            assert response["exited"] is False
    print("PASS guarded host: old protocol retained, stale attach/send rejected before resize/input, bound ACK/replay/input, same-name replacement rejected, free shells exited and reaped")


if __name__=="__main__":
    main()
