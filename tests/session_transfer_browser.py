#!/usr/bin/env python3
"""Transfer foundations: real Rust planner/stager and Chromium history reads.

Synthetic roots only, no model calls, native publish, source deletion or unit
tests. Staged clones remain explicitly non-publishable pending native adapters.
"""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, Corpus, batch35_meta, codex_message, isolated_server, get_json


def ident(n):
    return f'10000000-0000-4000-8000-{n:012d}'


def command(binary, request, error=None):
    result = subprocess.run([str(binary)], input=json.dumps(request), text=True,
                            capture_output=True, timeout=30)
    data = json.loads(result.stdout)
    if error:
        assert result.returncode and data['error']['code'] == error, (result, data)
    else:
        assert result.returncode == 0, (result.stderr, data)
    return data


def fingerprint(root):
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in root.rglob('*') if p.is_file()}


def fixture(root, cwd=None):
    corpus = Corpus(root)
    for source in ('claude', 'codex', 'grok'):
        (root / source).mkdir(parents=True)

    def put(key, sid, rollout, hour, text, ordinal, parent=None, inherited=None, agent=False, archived=False):
        extra = {"cwd": str(cwd)} if cwd else {}
        if parent:
            if agent:
                extra['source'] = {'subagent': {'thread_spawn': {'parent_thread_id': parent}}}
                extra['parent_thread_id'] = parent
            else:
                extra['forked_from_id'] = parent
        if inherited:
            source, physical, end = inherited
            extra['history_base'] = {'thread_id': physical, 'end_byte_offset': source.stat().st_size,
                                     'end_ordinal_exclusive': end}
        meta = batch35_meta(sid, f'2026-09-11T{hour:02d}:00:00Z', **extra)
        meta['ordinal'] = ordinal
        rows = [meta, codex_message('user', text, ordinal + 1),
                codex_message('assistant', text + ' answer', ordinal + 2)]
        path = root / 'codex' / ('archived_sessions' if archived else 'sessions')
        path.mkdir(exist_ok=True)
        suffix = '' if sid == rollout else '_' + rollout
        path = path / f'rollout-2026-09-11T{hour:02d}-00-00-{sid}{suffix}.jsonl'
        # Spaces and Unicode deliberately make output line lengths differ.
        path.write_bytes(b''.join((json.dumps(row, ensure_ascii=False) + '\n').encode() for row in rows))
        corpus.paths[key] = path
        return path

    p = put('parent', ident(1), ident(1), 1, 'Common ancestor 原文 ' + ident(1), 0)
    a = put('a-old', ident(2), ident(2), 2, 'Branch A old', 3, ident(1), (p, ident(1), 3))
    current = put('a', ident(2), ident(3), 3, 'Branch A current', 6, ident(1), (a, ident(2), 6))
    put('b', ident(4), ident(4), 4, 'Branch B', 3, ident(1), (p, ident(1), 3), archived=True)
    put('grandchild', ident(5), ident(5), 5, 'Fork of revert', 9, ident(2), (current, ident(3), 9))
    with a.open('ab') as stream:
        stream.write((json.dumps(codex_message('assistant', 'Discarded old branch tail', 6)) + '\n').encode())
    put('parent-agent', ident(6), ident(6), 6, 'Parent agent', 0, ident(1), agent=True)
    put('a-agent', ident(7), ident(7), 7, 'A agent', 0, ident(2), agent=True)
    # Native event/response pairs share IDs; opaque user content keeps old IDs.
    events = [
        ('event_msg', {'type': 'task_started', 'turn_id': ident(20), 'root_turn_id': ident(21)}),
        ('response_item', {'type': 'function_call', 'id': 'fc_old', 'call_id': 'call_spawn',
                          'name': 'spawn_agent', 'arguments': json.dumps({'message': ident(6)})}),
        ('response_item', {'type': 'function_call_output', 'call_id': 'call_spawn',
                          'output': json.dumps({'agent_id': ident(6)})}),
        ('event_msg', {'type': 'collab_agent_spawn_end', 'call_id': 'call_spawn',
                       'sender_thread_id': ident(7), 'new_thread_id': ident(6), 'prompt': ident(6)}),
        ('event_msg', {'type': 'item_completed', 'thread_id': ident(7), 'turn_id': ident(20),
                       'item': {'type': 'CollabAgentToolCall', 'id': 'call_spawn',
                                'sender_thread_id': ident(7), 'receiver_thread_ids': [ident(6)],
                                'receiver_agents': [{'thread_id': ident(6)}],
                                'agents_states': {ident(6): {'completed': ident(6)}}}}),
        ('response_item', {'type': 'message', 'id': 'msg_old', 'role': 'assistant',
                          'content': [{'type': 'output_text', 'text': 'Identity reference check ' + ident(6)}],
                          'internal_chat_message_metadata_passthrough': {'turn_id': ident(20)}}),
        ('response_item', {'type': 'function_call', 'call_id': 'call_wait', 'name': 'wait',
                          'arguments': json.dumps({'ids': [ident(6)], 'timeout_ms': 1000})}),
        ('response_item', {'type': 'function_call_output', 'call_id': 'call_wait',
                          'output': json.dumps({'status': {ident(6): {'completed': ident(6)}}})}),
    ]
    with corpus.paths['a-agent'].open('a') as stream:
        for ordinal, (kind, payload) in enumerate(events, 3):
            stream.write(json.dumps({'timestamp': '2026-09-11T07:00:00Z', 'ordinal': ordinal,
                                     'type': kind, 'payload': payload}) + '\n')
    # An unrelated broken graph must not poison this component.
    put('unrelated', ident(8), ident(8), 8, 'Unrelated missing parent', 0, ident(99))
    return corpus


def environment_checks(transfer, root):
    cwd=root/'workspace';cwd.mkdir()
    (cwd/'.git').mkdir();(cwd/'.git/HEAD').write_text('first')
    (cwd/'.gitignore').write_text('ignored\n')
    (cwd/'ignored').write_text('ignored but required')
    (cwd/'untracked').write_text('worktree bytes')
    external=root/'attachment';external.write_bytes(b'attachment bytes')
    (cwd/'a-link').symlink_to(cwd/'untracked')
    (cwd/'outside').symlink_to(external)
    (cwd/'cycle').symlink_to(cwd, target_is_directory=True)
    inspect={'operation':'inspect_environment','cwd':str(cwd),'dependencies':[str(external)]}
    snapshot=command(transfer,inspect)
    assert 'untracked' in snapshot['entries'] and 'ignored' in snapshot['entries']
    assert str(external) in snapshot['dependencies']
    assert all('.git' not in Path(p).parts for p in snapshot['entries'])
    compare={'operation':'compare_environment','snapshot':snapshot}
    assert command(transfer,compare)['matches']
    (cwd/'.git/HEAD').write_text('different git metadata')
    assert command(transfer,compare)['matches']
    for path,changed in [(cwd/'ignored',b'same dirty status, different bytes'),(external,b'changed attachment')]:
        before=path.read_bytes();path.write_bytes(changed)
        command(transfer,compare,error='move_cwd_mismatch')
        command(transfer,{'operation':'recheck_environment','snapshot':snapshot},error='move_plan_stale')
        path.write_bytes(before)
    (cwd/'untracked').chmod(0o700)
    command(transfer,compare,error='move_cwd_mismatch')
    (cwd/'untracked').chmod(0o600)
    (cwd/'added').write_text('new path')
    command(transfer,compare,error='move_cwd_mismatch');(cwd/'added').unlink()
    assert command(transfer,compare)['matches']
    # Per-host inode and mtime differences are not content differences.
    changed_stamp=json.loads(json.dumps(snapshot))
    for stamp in changed_stamp['stamps'].values():stamp['inode']+=1
    assert command(transfer,{'operation':'compare_environment','snapshot':changed_stamp})['matches']
    source=root/'store';source.mkdir();target=root/'other-store';target.mkdir()
    alias=root/'same-store';alias.symlink_to(source,target_is_directory=True)
    probe=command(transfer,{'operation':'create_storage_probe','root':str(source)})
    assert command(transfer,{'operation':'check_storage_probe','root':str(alias),'probe':probe})['shared']
    assert not command(transfer,{'operation':'check_storage_probe','root':str(target),'probe':probe})['shared']
    command(transfer,{'operation':'remove_storage_probe','root':str(source),'probe':probe})
    assert not list(source.iterdir())
    print('PASS content preflight: ignored/untracked files, executable bits, external links, cycles, stale scan; shared storage nonce')
    return cwd


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--transfer-binary', type=Path)
    args = parser.parse_args()
    transfer = args.transfer_binary or args.binary.with_name('sessiondock-transfer')
    with tempfile.TemporaryDirectory(prefix='sessiondock-transfer-') as temporary:
        root = Path(temporary)
        cwd=environment_checks(transfer,root)
        source = fixture(root / 'source',cwd=cwd)
        roots = {'codex': str(source.root / 'codex')}
        original = fingerprint(source.root)
        wanted = {source.uid(key) for key in source.paths if key != 'unrelated'}
        for key in ('parent', 'a', 'b', 'grandchild', 'parent-agent', 'a-agent', 'a-old'):
            group = command(transfer, {'operation': 'group', 'uid': source.uid(key), 'roots': roots})
            assert {row['uid'] for row in group['members']} == wanted, (key, group)
            assert group['blockers'] == [], group
        broken = command(transfer, {'operation': 'group', 'uid': source.uid('unrelated'), 'roots': roots})
        assert broken['blockers'][0]['code'] == 'move_group_incomplete'
        print('PASS whole component from parent, siblings, agents and old rollout; unrelated broken group isolated')

        plans, staged = {}, {}
        for mode in ('move', 'clone'):
            plans[mode] = command(transfer, {'operation': 'plan_codex', 'mode': mode,
                'uid': source.uid('a'), 'roots': roots})
            destination = root / mode / 'codex'
            destination.parent.mkdir()
            for other in ('claude', 'grok'):
                (destination.parent / other).mkdir()
            staged[mode] = command(transfer, {'operation': 'stage_codex', 'plan': plans[mode],
                'destination': str(destination)})
            assert staged[mode]['publishable'] is False
            assert staged[mode]['required_checks']
            for file in staged[mode]['files']:
                raw = (destination / file['relative']).read_bytes()
                assert hashlib.sha256(raw).hexdigest() == file['sha256']
                if mode == 'move':
                    assert raw == Path(file['source']).read_bytes()
            assert fingerprint(source.root) == original
        clone = staged['clone']
        mapping = clone['identities']['threads']
        assert set(mapping).isdisjoint(mapping.values())
        assert clone['identities']['rollouts'][ident(2)] == mapping[ident(2)]
        assert clone['identities']['rollouts'][ident(3)] != mapping[ident(2)]
        assert any(int(old) != new for f in clone['files'] for old, new in f['boundaries'].items()), \
            'fixture did not exercise changed byte lengths'
        assert not plans['clone']['reference_issues']
        agent_file = next(f for f in clone['files'] if f['source'] == str(source.paths['a-agent']))
        native = [json.loads(line)['payload'] for line in
                  (root / 'clone' / 'codex' / agent_file['relative']).read_text().splitlines()][3:]
        records = clone['identities']['records']
        assert native[0]['root_turn_id'] == clone['identities']['turns'][ident(21)]
        assert native[1]['call_id'] == native[2]['call_id'] == native[3]['call_id'] == records['call_spawn']
        assert native[4]['item']['id'] == records['call_spawn']
        assert json.loads(native[1]['arguments'])['message'] == ident(6)
        assert json.loads(native[2]['output'])['agent_id'] == mapping[ident(6)]
        assert native[3]['prompt'] == ident(6) and native[3]['new_thread_id'] == mapping[ident(6)]
        assert native[4]['thread_id'] == mapping[ident(7)]
        assert native[4]['item']['receiver_agents'][0]['thread_id'] == mapping[ident(6)]
        assert native[4]['item']['agents_states'] == {mapping[ident(6)]: {'completed': ident(6)}}
        assert native[5]['id'] == records['msg_old']
        assert native[5]['internal_chat_message_metadata_passthrough']['turn_id'] == clone['identities']['turns'][ident(20)]
        assert json.loads(native[6]['arguments'])['ids'] == [mapping[ident(6)]]
        assert json.loads(native[7]['output'])['status'] == {mapping[ident(6)]: {'completed': ident(6)}}
        print('PASS native call/result/event identities, nested agent references, root turns and untouched text')
        print('PASS byte-exact move staging; clone identities and recomputed boundaries; source unchanged')

        # The same confirmed plan deterministically produces the same identity
        # and content in a new staging attempt, with no second identity allocation.
        again = command(transfer, {'operation': 'stage_codex', 'plan': plans['clone'],
            'destination': str(root / 'retry')})
        assert again == clone
        command(transfer, {'operation': 'stage_codex', 'plan': plans['clone'],
            'destination': str(root / 'retry')}, error='move_io')
        invalid = copy.deepcopy(plans['clone'])
        invalid['identities']['threads'].pop(ident(1))
        command(transfer, {'operation': 'stage_codex', 'plan': invalid,
            'destination': str(root / 'invalid')}, error='move_identity')
        command(transfer, {'operation': 'stage_codex', 'plan': plans['clone'],
            'destination': str(source.root / 'codex' / 'bad-stage')}, error='move_path')

        with sync_playwright() as playwright:
            launch = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = playwright.chromium.launch(**launch)
            try:
                for mode in ('move', 'clone'):
                    corpus = Corpus(root / mode)
                    with isolated_server(corpus, args.binary) as (base, opener):
                        rows = get_json(opener, base, '/api/sessions')['sessions']
                        ids = plans[mode]['identities']['threads']
                        current = next(row for row in rows if row['sid'] == ids[ident(2)] and not row.get('continued_in'))
                        grandchild = next(row for row in rows if row['sid'] == ids[ident(5)])
                        assert all(row['supported'] for row in rows), rows
                        assert {a['id'] for a in current['agent_items']} == {ids[ident(7)]}
                        context = browser.new_context(service_workers='block')
                        context.route('**/*', lambda route: route.continue_()
                                      if route.request.url.startswith(base + '/') else route.abort())
                        page = context.new_page()
                        page.goto(base, wait_until='networkidle')
                        page.locator(f'#side .item[data-uid="{grandchild["uid"]}"]').click()
                        expect(page.locator('#msgs')).to_contain_text('Fork of revert')
                        for text in ('Branch A current', 'Branch A old', 'Common ancestor 原文 ' + ident(1)):
                            expect(page.locator('#msgs')).to_contain_text(text)
                        expect(page.locator('#msgs')).not_to_contain_text('Discarded old branch tail')
                        page.locator(f'#side .item[data-uid="{current["uid"]}"]').click()
                        expect(page.locator('#msgs')).to_contain_text('Branch A current')
                        page.locator('#a-view-switch').click()
                        page.locator(f'#session-view-menu button[data-agent="{ids[ident(7)]}"]').click()
                        expect(page.locator('#msgs')).to_contain_text('A agent answer')
                        expect(page.locator('#msgs')).to_contain_text('Identity reference check ' + ident(6))
                        context.close()
                        print(f'PASS Chromium opens {mode} staging: inherited history, untouched literal UUID, remapped agent menu')
            finally:
                browser.close()
        with source.paths['a-agent'].open('a') as stream:
            stream.write(json.dumps({'type': 'response_item', 'ordinal': 11, 'payload': {
                'type': 'custom_tool_call', 'call_id': 'call_exec', 'name': 'exec',
                'input': 'await tools.send_input({target: "' + ident(6) + '"})'}}) + '\n')
        audited = command(transfer, {'operation': 'plan_codex', 'mode': 'clone',
            'uid': source.uid('a'), 'roots': roots})
        assert len(audited['reference_issues']) == 1
        assert 'code-mode' in audited['reference_issues'][0]['reason']
        print('PASS embedded code references produce explicit planning diagnostics')
        # A source change after confirmation cannot produce a successful stage.
        with source.paths['parent'].open('ab') as stream:
            stream.write(b'{}\n')
        command(transfer, {'operation': 'stage_codex', 'plan': plans['clone'],
            'destination': str(root / 'stale')}, error='move_plan_stale')
        assert not (root / 'stale' / 'manifest.json').exists()
        print('PASS stable retry, collision refusal, malformed plan, staging confinement and stale-source rejection')


if __name__ == '__main__':
    main()
