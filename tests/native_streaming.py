#!/usr/bin/env python3
"""Real HTTP regression for checked streaming native JSONL reads.

Uses only generated temporary Claude/Codex/Grok records and the explicit Rust
binary on loopback. Native files are changed only by this test's owned mutation
helper; every HTTP response is followed by an exact byte-integrity check.
No Python adapters, CLI, model request, or production input is used.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import tempfile
from urllib.error import HTTPError

from history_parity import BINARY, Corpus, claude_row, codex_row, cursor_query, encoded, get_json, isolated_server


def uid(corpus,sid):
    source=sid.split("-",1)[0]
    return source+":"+hashlib.sha1(str(corpus.paths[sid]).encode()).hexdigest()[:16]


def message(source, sid, index, text):
    role="user" if index%2==0 else "assistant"
    if source=="claude":
        return claude_row(sid,role,f"row-{index}",f"row-{index-1}" if index else None,text)
    if source=="codex":
        return codex_row("response_item",{"type":"message","role":role,
            "content":[{"type":"input_text" if role=="user" else "output_text","text":text}]})
    return {"type":role,"prompt_index":index//2+1,"content":[{"type":"text","text":text}]}


def unreadable(source, sid, index):
    """A complete record the Python adapters raise on too: scalar content."""
    row=message(source, sid, index, "")
    if source=="claude":
        row["message"]["content"]=42
    elif source=="codex":
        row["payload"]["content"]=42
    else:
        row["content"]=42
    return row


def meta(sid,**extra):
    return codex_row("session_meta",{"id":sid,"session_id":sid,"cwd":"/synthetic/native-stream",**extra})


def put(corpus,source,sid,rows):
    if source!="grok":
        corpus.put(sid,source,rows,[])
        return corpus.paths[sid]
    path=corpus.root/"grok/project-native-stream"/sid
    path.mkdir(parents=True)
    (path/"summary.json").write_text(json.dumps({"info":{"id":sid,"cwd":"/synthetic/native-stream"}}))
    (path/"chat_history.jsonl").write_bytes(b"".join(encoded(row) for row in rows))
    corpus.paths[sid]=path
    return path/"chat_history.jsonl"


def build(root):
    corpus=Corpus(root)
    for source in ("claude","codex","grok"):
        (root/source).mkdir()
    cases={}
    for source in ("claude","codex","grok"):
        sid=source+"-stream"
        texts=[source+" PAD "+"x"*6000,source+" PREFIX OLD",source+" LAST COMMITTED"]
        rows=[message(source,sid,index,text) for index,text in enumerate(texts)]
        path=put(corpus,source,sid,([meta(sid)] if source=="codex" else [])+rows)
        # Mix physical blank lines, LF and CRLF. The unfinished suffix is split
        # inside an actual UTF-8 scalar rather than at a convenient JSON boundary.
        committed=(encoded(meta(sid)) if source=="codex" else b"")+b"\n \t\r\n"
        committed+=encoded(rows[0])+b"\r\n"+encoded(rows[1]).replace(b"\n",b"\r\n")
        committed+=b"\t\n"+encoded(rows[2])
        half_text=source+" HALF 半行 😀"
        half=encoded(message(source,sid,3,half_text))[:-1]
        split=half.index("😀".encode())+2
        path.write_bytes(committed+half[:split])
        cases[source]={"sid":sid,"path":path,"texts":texts,"committed":len(committed),
            "rest":half[split:],"half_text":half_text}

        order_sid=source+"-order"
        arguments={"z":"first","a":False,"r":4,"b":"last","extra":"omitted"}
        start=message(source,order_sid,0,"ORDER INPUT")
        if source=="claude":
            call=claude_row(order_sid,"assistant","row-1","row-0",[
                {"type":"tool_use","id":"synthetic-order-call","name":"unknown_named_tool","input":arguments}])
            order_rows=[start,call]
        elif source=="codex":
            order_rows=[meta(order_sid),start,codex_row("response_item",{"type":"function_call",
                "name":"unknown_named_tool","call_id":"synthetic-order-call","arguments":json.dumps(arguments)})]
        else:
            order_rows=[start,{"type":"assistant","content":"","tool_calls":[
                {"id":"synthetic-order-call","name":"unknown_named_tool","arguments":arguments}]}]
        put(corpus,source,order_sid,order_rows)

    parent="codex-stream-parent";leaf="codex-stream-leaf"
    inherited=["PARENT PAD "+"p"*6000,"PARENT PREFIX OLD"]
    prefix=encoded(meta(parent))+b"\r\n"+encoded(message("codex",parent,0,inherited[0]))
    prefix+=encoded(message("codex",parent,1,inherited[1])).replace(b"\n",b"\r\n")+b" \t\n"
    parent_path=put(corpus,"codex",parent,[])
    parent_path.write_bytes(prefix+encoded(message("codex",parent,2,"PARENT OUTSIDE CUT")))
    leaf_texts=["LEAF ONLY","LEAF ANSWER"]
    leaf_path=put(corpus,"codex",leaf,[meta(leaf,forked_from_id=parent,
        history_base={"thread_id":parent,"end_byte_offset":len(prefix)}),
        *[message("codex",leaf,index,text) for index,text in enumerate(leaf_texts)]])
    return corpus,cases,{"parent":parent,"leaf":leaf,"parent_path":parent_path,"leaf_path":leaf_path,
        "cut":len(prefix),"inherited":inherited,"leaf_texts":leaf_texts}


class OwnedFiles:
    def __init__(self,root):
        self.root=root
        self.expected={path:path.read_bytes() for path in root.rglob('*') if path.is_file()}

    def unchanged(self):
        assert {path for path in self.root.rglob('*') if path.is_file()}==set(self.expected),"backend added or removed a native fixture"
        for path,expected in self.expected.items():
            assert path.read_bytes()==expected,"backend changed an owned native fixture"

    def write(self,path,data,*,restore_mtime=False):
        self.unchanged()
        assert path in self.expected
        previous=path.stat()
        path.write_bytes(data)
        if restore_mtime:
            os.utime(path,ns=(previous.st_atime_ns,previous.st_mtime_ns))
            assert path.stat().st_mtime_ns==previous.st_mtime_ns
        self.expected[path]=data

    def append(self,path,data):
        self.write(path,self.expected[path]+data)


def texts(response):
    return [row["text"] for row in response["messages"]]


def run(corpus,cases,fork,base,opener,owned):
    def read(sid,previous=None):
        route="/api/messages/"+uid(corpus,sid)
        if previous is not None:
            route+="?"+cursor_query(previous)
        try:
            return get_json(opener,base,route)
        finally:
            owned.unchanged()

    def reject(sid,previous=None):
        try:
            read(sid,previous)
        except HTTPError as error:
            assert error.code==501,(sid,error.code)
            body=json.loads(error.read(8193))
            assert isinstance(body.get("error"),str) and body["error"]
            assert "messages" not in body,"malformed native input returned partial success"
            assert str(corpus.root) not in json.dumps(body),"error exposed fixture paths"
            return
        raise AssertionError("malformed complete native record was silently accepted")

    for source,case in cases.items():
        sid,path=case["sid"],case["path"]
        expected=list(case["texts"])
        initial=read(sid)
        assert texts(initial)==expected and initial["end"]==case["committed"]
        idle=read(sid,initial)
        assert idle["reset"] is False and texts(idle)==[] and idle["end"]==initial["end"]

        owned.append(path,case["rest"])
        pending=read(sid,initial)
        assert pending["reset"] is False and texts(pending)==[] and pending["end"]==initial["end"]
        owned.append(path,b"\r")
        pending_cr=read(sid,pending)
        assert pending_cr["reset"] is False and texts(pending_cr)==[] and pending_cr["end"]==initial["end"]
        owned.append(path,b"\n")
        current=read(sid,pending_cr)
        expected.append(case["half_text"])
        assert current["reset"] is False and texts(current)==[case["half_text"]]
        assert current["end"]==len(owned.expected[path])
        assert texts(read(sid))==expected,"append cache lost earlier records"

        owned.append(path,b"\n \t\r\n\n")
        blank=read(sid,current)
        assert blank["reset"] is False and texts(blank)==[]
        assert blank["end"]==len(owned.expected[path]) and blank["end"]>current["end"]
        current=blank
        for index in range(4,7):
            text=f"{source} APPEND {index}"
            row=encoded(message(source,sid,index,text))
            owned.append(path,row if index%2 else row.replace(b"\n",b"\r\n"))
            delta=read(sid,current)
            assert delta["reset"] is False and texts(delta)==[text]
            expected.append(text)
            assert texts(read(sid))==expected
            assert delta["end"]==len(owned.expected[path])
            current=delta

        before=owned.expected[path]
        marker=(source+" PREFIX OLD").encode()
        assert before.index(marker)>4096
        changed=before.replace(marker,(source+" PREFIX NEW").encode(),1)
        assert changed[:4096]==before[:4096] and len(changed)==len(before)
        owned.write(path,changed,restore_mtime=True)
        rewritten=read(sid,current)
        expected[1]=source+" PREFIX NEW"
        assert rewritten["reset"] is True and texts(rewritten)==expected
        assert rewritten["end"]==current["end"] and rewritten["version"]["head"]==current["version"]["head"]
        text=source+" POST REWRITE"
        owned.append(path,encoded(message(source,sid,7,text)))
        current=read(sid,rewritten)
        assert current["reset"] is False and texts(current)==[text]
        expected.append(text)
        assert texts(read(sid))==expected

        clean=owned.expected[path]
        # A malformed complete line is skipped while keeping its bytes in the
        # physical cursor.
        malformed=b'{"type":]\n'
        owned.write(path,clean+malformed)
        noted=read(sid,current)
        assert noted["reset"] is False and texts(noted)==[] and noted["end"]==len(clean)+len(malformed)
        owned.write(path,clean)
        restored=read(sid)
        assert texts(restored)==expected and restored["end"]==len(clean)

        owned.write(path,clean+encoded(unreadable(source,sid,9)))
        reject(sid);reject(sid,current)
        owned.write(path,clean)
        restored=read(sid)
        assert texts(restored)==expected and restored["end"]==len(clean)
        order=read(source+"-order")
        assert any(row.get("summary")=="z=first a=False r=4 b=last" for row in order["messages"]),"tool argument key order changed"

    # An inherited fixed prefix has independent authority/semantics. Public
    # end remains the physical leaf checkpoint, never parent+leaf byte totals.
    leaf,parent=fork["leaf"],fork["parent"]
    leaf_path,parent_path=fork["leaf_path"],fork["parent_path"]
    current=read(leaf)
    expected=fork["inherited"]+fork["leaf_texts"]
    assert texts(current)==expected
    assert current["end"]==len(owned.expected[leaf_path]) and current["end"]<fork["cut"]
    before=owned.expected[parent_path]
    assert before.index(b"PARENT PREFIX OLD")>4096
    changed=before.replace(b"PARENT PREFIX OLD",b"PARENT PREFIX NEW",1)
    assert changed[:4096]==before[:4096] and len(changed)==len(before)
    owned.write(parent_path,changed,restore_mtime=True)
    rewritten=read(leaf,current)
    expected[1]="PARENT PREFIX NEW"
    assert rewritten["reset"] is True and texts(rewritten)==expected
    assert rewritten["end"]==current["end"]
    current=rewritten
    owned.append(parent_path,encoded(message("codex",parent,3,"PARENT APPEND OUTSIDE CUT")))
    delta=read(leaf,current)
    assert delta["reset"] is False and texts(delta)==[] and delta["end"]==current["end"]
    owned.append(parent_path,encoded(unreadable("codex",parent,9)))
    reject(parent)
    delta=read(leaf,current)
    assert delta["reset"] is False and texts(delta)==[] and delta["end"]==current["end"],"parent tail after cut poisoned inherited view"
    owned.append(leaf_path,encoded(message("codex",leaf,2,"LEAF NEW")))
    delta=read(leaf,current)
    assert delta["reset"] is False and texts(delta)==["LEAF NEW"]
    assert delta["end"]==len(owned.expected[leaf_path])
    assert texts(read(leaf))==expected+["LEAF NEW"]
    owned.unchanged()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary",type=Path,default=BINARY)
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-native-stream-") as temporary:
        corpus,cases,fork=build(Path(temporary))
        owned=OwnedFiles(corpus.root)
        with isolated_server(corpus,args.binary) as (base,opener):
            run(corpus,cases,fork,base,opener,owned)
        owned.unchanged()
        print("PASS native streaming HTTP: three providers LF/CRLF/blank/partial UTF8 lines; append preservation; >4KiB same-size restored-mtime rewrite reset; malformed/duplicate repair; ordered tool args; parent fixed cut and leaf-only end; native bytes unchanged")


if __name__=="__main__":
    main()
