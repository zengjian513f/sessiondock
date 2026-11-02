#!/usr/bin/env python3
"""Kiro large tool output + Unicode stream: real node/Hub SSE bytes and recovery."""
import argparse
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY
from hub_fixtures import Hub, free_port
from native_sdk_rename_browser import native
from node_auth_fixtures import TOKEN, node_env
from sdk_ui_fixtures import start_sdk, reveal_sdk_tool
from sdkhost_fixtures import stop_private_sdkhosts

SID = '019a0000-0000-7000-8000-000000000301'
METER = r'''(() => {
  const Original = window.EventSource;
  window.sdkWire = [];
  window.EventSource = class extends Original {
    constructor(url, options) {
      super(url, options);
      if (String(url).includes('/native-sdk/events')) Original.prototype.addEventListener.call(this, 'sdk', event => {
        const packet = JSON.parse(event.data);
        window.sdkWire.push({type:packet.type, bytes:new TextEncoder().encode(event.data).length,
          version:packet.snapshot_version, base:packet.snapshot_base});
      });
    }
    addEventListener(type, callback, options) {
      if (type !== 'sdk') return super.addEventListener(type, callback, options);
      return super.addEventListener(type, event => {
        if (window.dropSdkDelta && JSON.parse(event.data).type === 'snapshot_delta') {
          window.dropSdkDelta = false;
          return;
        }
        callback.call(this, event);
      }, options);
    }
  };
})();'''


def prompt_count(root):
    path = root / 'kiro-acp-received.jsonl'
    return sum(json.loads(line).get('method') == 'session/prompt' for line in path.read_text().splitlines())


def exercise(browser, base, root, shots, label):
    context = browser.new_context(viewport={'width':1280, 'height':900}, service_workers='block')
    context.add_init_script(METER)
    page = context.new_page()
    page.set_default_timeout(30000)
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    directory = root / 'fake-native-store'
    output = '工具日志🙂起点\n' + ''.join(f'line {i:06d}: synthetic tool output; keep every byte.\n' for i in range(22000)) + '完整输出终点\n'
    assert len(output.encode()) > 1_000_000
    (directory / 'delta-output.txt').write_text(output)
    release = directory / 'delta-release'
    release.write_text('-1')
    page.goto(base, wait_until='domcontentloaded')
    uid = page.wait_for_function('''async sid => {await loadSessions(true);
      return S.sessions.find(row => row.sid === sid)?.uid;}''', arg=SID, polling=500).json_value()
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    page.locator('#a-sdk').click()
    start_sdk(page)
    before = prompt_count(root)
    box = page.locator('#sdk-input')
    box.fill('delta-bandwidth ' + label)
    page.locator('#sdk-send').click()
    page.wait_for_function('''() => NativeSdkView.running &&
      JSON.stringify(NativeSdkView.raw.items).includes('完整输出终点')''')
    # The live turn is complete even though it exceeds the history-window budget.
    assert page.evaluate('text => JSON.stringify(NativeSdkView.raw.items).includes(text)', 'line 021999:')
    start = page.evaluate('sdkWire.length')
    for step in range(20):
        release.write_text(str(step))
        page.wait_for_function('text => NativeSdkView.items.some(i => i.role === "assistant" && i.text?.includes(text))',
                               arg=f'增量{step:02d}界🙂')
    frames = page.evaluate('start => sdkWire.slice(start)', start)
    assert any(f['type'] == 'snapshot_delta' for f in frames), frames
    assert not any(f['type'] == 'snapshot' for f in frames), frames
    size = sum(f['bytes'] for f in frames)
    assert size < 160_000, (label, size, frames)
    # Unicode correction/deletion does not duplicate a surrogate or retain old text.
    tool = page.evaluate('''() => JSON.stringify(NativeSdkView.raw.items.filter(i => i.role === 'tool'))''')
    assert '🙃已修订' in tool and '🙂待修订' not in tool
    assert 'line 021999:' in tool
    tool_node = page.locator('.sdk-live .tool-msg').filter(has_text='read synthetic log').last
    reveal_sdk_tool(tool_node)
    expect(tool_node.locator('.tool-out')).to_contain_text('工具日志🙂起点')
    print(f'{label}: {len(output.encode())} byte tool output retained; 20 updates sent {size} bytes ({len(frames)} frames)', flush=True)

    # A missing delta must recover by snapshot, never silently lose a chunk or resend input.
    full_before = page.evaluate('sdkWire.filter(f => f.type === "snapshot").length')
    page.evaluate('window.dropSdkDelta = true')
    release.write_text('20')
    page.wait_for_function('!window.dropSdkDelta')
    release.write_text('21')
    page.wait_for_function('text => NativeSdkView.items.some(i => i.text?.includes(text))', arg='增量21界🙂')
    page.wait_for_function('n => sdkWire.filter(f => f.type === "snapshot").length > n', arg=full_before)
    assert prompt_count(root) == before + 1
    release.write_text('22')
    page.wait_for_function('text => NativeSdkView.items.some(i => i.text?.includes(text))', arg='增量22界🙂')
    page.reload(wait_until='domcontentloaded')
    page.wait_for_function('NativeSdkView.connected && NativeSdkView.running')
    page.wait_for_function('text => NativeSdkView.items.some(i => i.text?.includes(text))', arg='增量22界🙂')
    assert page.evaluate('text => JSON.stringify(NativeSdkView.raw.items).includes(text)', '完整输出终点')
    assert prompt_count(root) == before + 1

    # An old page's ordinary subscription still receives complete snapshots.
    legacy = page.evaluate('''uid => new Promise((resolve,reject) => {
      const es = new EventSource('api/native-sdk/events?'+new URLSearchParams({uid}));
      es.addEventListener('sdk', event => {const packet=JSON.parse(event.data);es.close();
        resolve({type:packet.type,version:packet.snapshot_version,
          complete:JSON.stringify(packet.raw.items).includes('完整输出终点')});});
      es.onerror = () => {es.close();reject(Error('legacy stream failed'));};
    })''', uid)
    assert legacy['type'] == 'snapshot' and legacy['complete'] and legacy.get('version') is None, legacy
    # A new page can also work with an old node that ignores the opt-in query.
    def old_node(route):
        from urllib.parse import urlsplit, parse_qsl, urlencode, urlunsplit
        u = urlsplit(route.request.url)
        route.continue_(url=urlunsplit((u.scheme,u.netloc,u.path,
            urlencode([(k,v) for k,v in parse_qsl(u.query) if k!='snapshot_delta']),u.fragment)))
    page.route('**/api/native-sdk/events?*', old_node)
    page.reload(wait_until='domcontentloaded')
    page.wait_for_function('NativeSdkView.connected && NativeSdkView.running')
    release.write_text('23')
    page.wait_for_function('text => NativeSdkView.items.some(i => i.text?.includes(text))', arg='增量23界🙂')
    assert page.evaluate('sdkWire.some(f => f.type === "snapshot" && f.version === undefined)')
    page.unroute('**/api/native-sdk/events?*', old_node)
    page.reload(wait_until='domcontentloaded')
    page.wait_for_function('NativeSdkView.connected && NativeSdkView.running')
    for step in range(24,28):
        release.write_text(str(step))
        page.wait_for_function('text => NativeSdkView.items.some(i => i.text?.includes(text))', arg=f'增量{step:02d}界🙂')
    page.wait_for_function('!NativeSdkView.running')
    expected = ''.join(f' 增量{step:02d}界🙂' for step in range(28))
    assert page.evaluate('text => NativeSdkView.items.some(i => i.role === "assistant" && i.text === text)', expected)
    assert prompt_count(root) == before + 1
    assert page.evaluate('text => JSON.stringify(NativeSdkView.raw.items).includes(text)', 'line 021999:')
    for width, theme in ((1280,'light'), (390,'dark')):
        page.set_viewport_size({'width':width,'height':900})
        page.emulate_media(color_scheme=theme)
        if width == 390:
            page.evaluate('showMobileDetail()')
        expect(page.locator('#sdk-input')).to_be_visible()
        expect(page.locator('.sdk-live')).to_contain_text('增量27界🙂')
        if shots:
            page.screenshot(path=str(shots / f'kiro-delta-{label}-{width}-{theme}.png'), full_page=True)
    # A new user turn drops the old live window; the old turn remains in history.
    box.fill('after-large-delta-turn')
    page.locator('#sdk-send').click()
    page.wait_for_function('NativeSdkView.raw.last_turn?.status === "completed" && !NativeSdkView.running')
    expect(page.locator('.sdk-live')).to_contain_text('Kiro ACP V3 final answer for: after-large-delta-turn')
    assert not page.evaluate('text => JSON.stringify(NativeSdkView.raw.items).includes(text)', 'line 021999:')
    assert not errors, errors
    context.close()
    print(f'PASS {label}: Unicode, window removal, lost delta, reload, old/new clients, complete output and one native prompt', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='kiro-delta-') as temporary, sync_playwright() as pw:
        root = Path(temporary)
        port = free_port()
        browser = pw.chromium.launch(headless=True)
        try:
            with native('kiro',root,args.binary,node_env(root,port,'127.0.0.0/8')) as base:
                exercise(browser,base,root,args.screenshots,'node')
                hubroot = root / 'hub'
                hubroot.mkdir()
                hub = Hub(args.binary.with_name('sessiondock-hub').resolve(),hubroot,
                    [SimpleNamespace(name='DeltaNode',token=TOKEN,port=port)])
                hub.start()
                try:
                    exercise(browser,f'http://127.0.0.1:{hub.port}',root,args.screenshots,'hub')
                finally:
                    hub.stop()
        finally:
            stop_private_sdkhosts(root)
            browser.close()


if __name__ == '__main__':
    main()
