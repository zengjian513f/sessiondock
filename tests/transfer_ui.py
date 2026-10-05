"""Shared user interaction with the transfer dialog's machine picker."""
import json


def select_target(dialog, node_id):
    dialog.locator('#transfer-target').click()
    dialog.locator(f'#transfer-target-options [data-value="{node_id}"]').click()


def seed_names(home, sid):
    title = '自定义标题 保留原名'
    raw = (' { "thread_name" : '+json.dumps(title)+', "id" : "'+sid+'",'
           ' "updated_at" : "2026-10-02T04:21:48Z", "extra" : "literal '+sid+'" }\r\n').encode()
    before = b'not-json\n'+json.dumps({'id':'unrelated','thread_name':'Keep unrelated'}).encode()+b'\n'
    before += json.dumps({'id':sid,'thread_name':'Earlier title'}).encode()+b'\n'+raw
    (home/'session_index.jsonl').write_bytes(before)
    return title, raw, before
