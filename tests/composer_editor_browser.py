#!/usr/bin/env python3
"""Local composer user paths against a private fake Claude PTY, never a model.

Requires an already-built sessiondock binary and target/debug/ptyhost. Select
the built Vue entry with SESSIONDOCK_TEST_WEB_DIR; unset it for legacy-web.
Uses send_browser's lifecycle/history helpers and history_parity's loopback
server. Two ordinary UI sends populate native input history; no protocol
assertions or injected product state. Only IME events are explicitly simulated
(Chromium automation cannot drive an OS IME); this checks the KeyboardEvent
guard, not real candidate selection. Mobile is a 390px Chromium viewport,
not a device soft keyboard. The file chooser selects a tiny synthetic file.
"""
import argparse
import json
import os
import sys
import tempfile
from pathlib import Path
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright

from browser_runtime import js
from history_parity import BINARY, REPO, Corpus, isolated_server
from popups import on_popup
from send_browser import FAKE_CLI, initialize, wait_history


def fixture(root, binary):
    for name in ('host', 'work', 'work/claude-area', 'ledger', 'state', 'home',
                 'claude', 'codex', 'grok'):
        (root / name).mkdir(mode=0o700)
    launcher = root / 'launcher.json'
    launcher.touch(mode=0o600)
    launcher.write_text(json.dumps({'schema': 2,
        'host_binary': str(REPO / 'target/debug/ptyhost'), 'host_dir': str(root / 'host'),
        'adapters': [], 'profiles': [{'id': 'claude-cli-v1', 'source': 'claude',
            'executable': str(Path(sys.executable).resolve()),
            'args': [str(FAKE_CLI), '--reply'],
            'new_args': ['--session-id', '{session_id}'],
            'resume_args': ['--resume', '{sid}'],
            'env': {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'),
                'TERM': 'xterm-256color', 'LANG': 'C.UTF-8',
                'SESSIONDOCK_TEST_CLAUDE_ROOT': str(root / 'claude')}}]}))
    initialize('--initialize-lifecycle', root / 'ledger', binary)
    return launcher


def type_text(editor, text):
    editor.click()
    editor.press('ControlOrMeta+A')
    editor.press('Backspace')
    if text:
        editor.press_sequentially(text)


def composing_key(editor, key):
    # Untrusted events do not perform native editing. Check the handler's
    # cancellation plus the visible UI separately, with actual Enter as control.
    result = editor.evaluate("""(el, key) => {
        el.dispatchEvent(new CompositionEvent('compositionstart', {bubbles:true, data:'测'}));
        const event = new KeyboardEvent('keydown', {
            key, code:key, bubbles:true, cancelable:true, isComposing:true});
        el.dispatchEvent(event);
        el.dispatchEvent(new CompositionEvent('compositionend', {bubbles:true, data:'测'}));
        return {composing:event.isComposing, prevented:event.defaultPrevented};
    }""", key)
    assert result == {'composing': True, 'prevented': False}, result


def history_paths(page, editor):
    picker = page.locator('#input-history')
    options = picker.locator('[role=option]')
    selected = picker.locator('[role=option][aria-selected=true]')
    type_text(editor, '')
    editor.press('ArrowUp')
    expect(picker).to_be_visible()
    expect(options).to_have_count(2)
    expect(selected).to_contain_text('editor history second')
    expect(editor).to_have_attribute('aria-expanded', 'true')
    expect(editor).to_have_attribute('aria-activedescendant', 'input-history-option-1')
    composing_key(editor, 'ArrowUp')
    composing_key(editor, 'Enter')
    expect(picker).to_be_visible()
    expect(selected).to_contain_text('editor history second')
    expect(editor).to_have_value('')
    editor.press('ArrowUp')
    expect(selected).to_contain_text('editor history first')
    editor.press('ArrowDown')
    expect(selected).to_contain_text('editor history second')
    editor.press('Enter')
    expect(picker).to_be_hidden()
    expect(editor).to_have_value('editor history second')
    expect(editor).to_be_focused()
    type_text(editor, '')
    editor.press('ArrowUp')
    expect(picker).to_be_visible()
    editor.press('Escape')
    expect(picker).to_be_hidden()
    expect(editor).to_have_value('')
    expect(editor).to_have_attribute('aria-expanded', 'false')
    editor.press('ArrowUp')
    expect(picker).to_be_visible()
    page.locator('#msgs').click(position={'x': 3, 'y': 3})
    expect(picker).to_be_hidden()
    editor.click()
    editor.press('ArrowUp')
    expect(picker).to_be_visible()
    editor.press_sequentially('editing closes history')
    expect(picker).to_be_hidden()
    expect(editor).to_have_value('editing closes history')


def attachments(page, editor):
    add, menu = page.locator('#cadd'), page.locator('#attach-menu')
    add.click()
    expect(menu).to_be_visible()
    expect(add).to_have_attribute('aria-expanded', 'true')
    editor.click()
    expect(menu).to_be_hidden()
    expect(add).to_have_attribute('aria-expanded', 'false')
    add.click()
    with page.expect_file_chooser() as chooser:
        menu.locator('[data-attach=file]').click()
    chooser.value.set_files({'name': 'editor-note.txt', 'mimeType': 'text/plain',
                             'buffer': b'synthetic composer attachment\n'})
    expect(menu).to_be_hidden()
    card = page.locator('#compose-items .draft-card').filter(has_text='editor-note.txt')
    expect(card).to_be_visible()
    type_text(editor, 'prefix OLD suffix')
    editor.press('Home')
    for _ in range(7):
        editor.press('ArrowRight')
    for _ in range(3):
        editor.press('Shift+ArrowRight')
    assert editor.evaluate('el => el.value.slice(el.selectionStart, el.selectionEnd)') == 'OLD'
    card.locator('.draft-info b').click()
    expect(editor).to_have_value('prefix [附件1] suffix')
    expect(editor).to_be_focused()
    editor.press_sequentially('?')
    expect(editor).to_have_value('prefix [附件1]? suffix')
    card.locator('.draft-remove').click()
    expect(card).to_have_count(0)


def sizing(page, editor):
    type_text(editor, 'line 0')
    measure = 'el => ({height:parseFloat(el.style.height), overflow:el.style.overflowY})'
    page.wait_for_function("document.querySelector('#cinput').style.overflowY === 'hidden'")
    baseline = editor.evaluate(measure)['height']
    for index in range(1, 3):
        editor.press('Shift+Enter')
        editor.press_sequentially(f'line {index}')
    expect(editor).to_have_value('line 0\nline 1\nline 2')
    page.wait_for_function("height => parseFloat(document.querySelector('#cinput').style.height) > height",
                           arg=baseline)
    for index in range(3, 24):
        editor.press('Shift+Enter')
        editor.press_sequentially(f'line {index}')
    page.wait_for_function("""() => {const el=document.querySelector('#cinput');
        return el.style.height === '180px' && el.style.overflowY === 'auto'
            && el.scrollHeight > el.clientHeight;}""")
    type_text(editor, 'short')
    page.wait_for_function("""height => {const el=document.querySelector('#cinput');
        return Math.abs(parseFloat(el.style.height)-height) < 1 && el.style.overflowY === 'hidden';} """,
                           arg=baseline)
    expect(editor).to_have_value('short')


def exercise(browser, base, root, mobile=False):
    context = browser.new_context(viewport={'width': 390 if mobile else 1280,
                                           'height': 844 if mobile else 900},
                                  service_workers='block')
    context.route('**/*', lambda route: route.continue_()
                  if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    errors, dialogs, receipt = [], [], None
    page.on('pageerror', lambda error: errors.append(str(error)))
    on_popup(page, lambda dialog: (dialogs.append(dialog.message), dialog.dismiss()))
    try:
        page.goto(base, wait_until='domcontentloaded')
        # Header folding can replace the inline button during actionability
        # checks. Let the live locator resolve whichever entry is visible.
        page.locator('#new-session:visible, #header-more-btn:visible').first.click()
        if not page.locator('#new-session-dialog').is_visible():
            page.locator('#new-session').click()
        page.locator('input[name="new-source"][value="claude"]').check()
        page.locator('#new-cwd').fill(str(root / 'work/claude-area'))
        # Capture only the cleanup identity; assertions below are UI assertions.
        with page.expect_response(lambda r: urlsplit(r.url).path == '/api/term/create') as created:
            page.locator('#new-session-go').click()
        receipt = created.value.json()
        editor = page.locator('#cinput')
        expect(editor).to_be_visible()
        expect(editor).to_be_enabled()
        page.evaluate('() => document.fonts.ready')
        page.wait_for_function(js('composerUid && !composerDraft()?.loading',
                                 'runtime.composer.composerUid && !runtime.composer.composerDraft()?.loading'))
        if mobile:
            type_text(editor, 'mobile first')
            expect(page.locator('#csend')).to_be_enabled()
            editor.press('Enter')
            editor.press_sequentially('mobile second')
            expect(editor).to_have_value('mobile first\nmobile second')
            expect(editor).to_be_focused()
        else:
            for index, label in enumerate(('first', 'second')):
                text = f'editor history {label}'
                type_text(editor, text)
                expect(page.locator('#csend')).to_be_enabled()
                if index == 0:
                    page.locator('#csend').click()
                else:
                    editor.press('Enter')
                expect(editor).to_have_value('')
                wait_history(page, text)
            type_text(editor, 'IME draft remains')
            expect(page.locator('#csend')).to_be_enabled()
            composing_key(editor, 'Enter')
            expect(editor).to_have_value('IME draft remains')
            expect(page.locator('#csend')).to_have_attribute('aria-busy', 'false')
            history_paths(page, editor)
            attachments(page, editor)
            sizing(page, editor)
        assert not errors, errors
        assert not dialogs, dialogs
        print(f"PASS composer editor {'mobile Enter' if mobile else 'desktop/history/IME/attachments/sizing'}", flush=True)
    finally:
        try:
            if receipt:
                context.request.post(base + '/api/term/kill', data={
                    'record_id': receipt['record_id'], 'instance_id': receipt['instance_id']})
        finally:
            context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    binary = parser.parse_args().binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-composer-editor-') as temporary:
        root = Path(temporary).resolve()
        launcher = fixture(root, binary)
        with isolated_server(Corpus(root), binary, host_dir=root / 'host',
                lifecycle_dir=root / 'ledger', launcher_config=launcher, state_dir=root / 'state',
                file_roots=(root / 'work',), file_write_roots=(root / 'work',)) as (base, _), sync_playwright() as pw:
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**options)
            try:
                exercise(browser, base, root)
                exercise(browser, base, root, mobile=True)
            finally:
                browser.close()


if __name__ == '__main__':
    main()
