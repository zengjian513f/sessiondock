"""Shared user interaction with the transfer dialog's machine picker."""
import json
import time


def select_target(dialog, node_id):
    dialog.locator('#transfer-target').click()
    dialog.locator(f'#transfer-target-options [data-value="{node_id}"]').click()


def wait_transfer(page, base, request, timeout=90, phases=('complete', 'aborted', 'cleanup_pending', 'aborting')):
    """Observe the durable job; never retry execution from a status waiter."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        response = page.request.post(base+'/api/session/transfer/progress', data=request)
        assert response.ok, response.text()
        status = response.json()
        if status['phase'] in phases and (status['phase'] in ('complete', 'aborted') or status.get('error')):
            return status
        page.wait_for_timeout(150)
    raise AssertionError(('transfer did not settle', status))


def submit_transfer(page, dialog, base, timeout=90):
    with page.expect_response('**/api/session/transfer/start', timeout=30000) as accepted:
        dialog.locator('.clone-confirm').click()
    response = accepted.value
    assert response.status == 202, response.text()
    return wait_transfer(page, base, response.json()['request'], timeout)


def seed_names(home, sid):
    title = '自定义标题 保留原名'
    raw = (' { "thread_name" : '+json.dumps(title)+', "id" : "'+sid+'",'
           ' "updated_at" : "2026-10-02T04:21:48Z", "extra" : "literal '+sid+'" }\r\n').encode()
    before = b'not-json\n'+json.dumps({'id':'unrelated','thread_name':'Keep unrelated'}).encode()+b'\n'
    before += json.dumps({'id':sid,'thread_name':'Earlier title'}).encode()+b'\n'+raw
    (home/'session_index.jsonl').write_bytes(before)
    return title, raw, before
