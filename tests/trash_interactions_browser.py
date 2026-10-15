#!/usr/bin/env python3
"""Recycle-bin controls use actual pointer/keyboard inputs and private disk receipts."""
import argparse
import json
from pathlib import Path
import tempfile

from playwright.sync_api import sync_playwright, expect
from history_fixtures import BINARY, Corpus, codex_row, codex_message, isolated_server


def confirm(page, accept):
    popup=page.locator('dialog.app-popup[open]')
    expect(popup).to_be_visible()
    popup.locator('[data-popup-action="'+('ok' if accept else 'cancel')+'"]').click()
    expect(popup).to_have_count(0)


def open_trash(page):
    if not page.locator('#trash').is_visible() and not page.locator('#header-menu').is_visible():
        page.locator('#header-more-btn').click()
    page.locator('#trash').click()
    expect(page.locator('#trash-dialog')).to_be_visible()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--output',type=Path,default=Path('target/trash-interactions'))
    args=parser.parse_args();args.output.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-trash-inputs-') as tmp, sync_playwright() as pw:
        root=Path(tmp);data=Corpus(root)
        for source in ['codex','claude','grok']:(root/source).mkdir(exist_ok=True)
        for i in range(3):
            sid=f'trash-ui-{i}'
            data.put(sid,'codex',[codex_row('session_meta',{'id':sid,'cwd':'/synthetic/trash'}),codex_message('user',f'Trash fixture {i}')],[])
        trash=root/'trash';trash.mkdir();native={sid:p.read_bytes() for sid,p in data.paths.items()}
        with isolated_server(data,args.binary,trash_dir=trash) as (base,_):
            browser=pw.chromium.launch(headless=True)
            try:
                page=browser.new_page(viewport={'width':1280,'height':800})
                page.goto(base,wait_until='networkidle')
                def delete(i,accept=True):
                    if page.locator('#detail .mobile-back').is_visible() and not page.locator('#side').is_visible():
                        page.locator('#detail .mobile-back').click()
                    page.locator('#side .item[data-uid="'+data.uid(f'trash-ui-{i}')+'"]').click()
                    if not page.locator('#a-session-action').is_visible():page.locator('#a-more').click()
                    page.locator('#a-session-action').click();confirm(page,accept)
                    if accept:
                        expect(page.locator('#detail')).to_contain_text('已移入回收站')
                        assert not data.paths[f'trash-ui-{i}'].exists()
                    else:assert data.paths[f'trash-ui-{i}'].read_bytes()==native[f'trash-ui-{i}']
                delete(0,False);delete(0)
                page.locator('#detail-open-trash').click()
                expect(page.locator('.trash-item')).to_have_count(1)
                entry=next(trash.iterdir());manifest=json.loads((entry/'manifest.json').read_text())
                assert manifest['uid']==data.uid('trash-ui-0') and manifest['state']=='trashed'
                # Another writer recreates the native file while the dialog is open.
                data.paths['trash-ui-0'].write_bytes(b'concurrent fixture')
                page.locator('.trash-item [data-act=restore]').click()
                expect(page.locator('#trash-note')).to_contain_text('原路径已存在')
                assert data.paths['trash-ui-0'].read_bytes()==b'concurrent fixture'
                page.locator('#trash-reload').click()
                expect(page.locator('.trash-item [data-act=restore]')).to_be_disabled()
                data.paths['trash-ui-0'].unlink()
                page.locator('#trash-reload').click()
                page.locator('.trash-item [data-act=restore]').click()
                expect(page.locator('#trash-list')).to_contain_text('没有已删除的会话')
                assert data.paths['trash-ui-0'].read_bytes()==native['trash-ui-0'] and not list(trash.iterdir())
                page.locator('#trash-done').click()
                for width in [1280,390]:
                    page.set_viewport_size({'width':width,'height':800})
                    page.wait_for_timeout(150)  # settle responsive menu relocation before clicking
                    for close in ['button','escape','backdrop','tab']:
                        open_trash(page)
                        if close=='button':page.locator('#trash-close').click()
                        elif close=='escape':page.keyboard.press('Escape')
                        elif close=='backdrop':page.mouse.click(2,2)
                        else:
                            for _ in range(15):
                                if page.locator('#trash-done').evaluate('e=>e===document.activeElement'):break
                                page.keyboard.press('Tab')
                            expect(page.locator('#trash-done')).to_be_focused();page.keyboard.press('Enter')
                        expect(page.locator('#trash-dialog')).to_be_hidden()
                for i in range(3):delete(i)
                open_trash(page)
                expect(page.locator('.trash-item')).to_have_count(3)
                before={p.relative_to(trash):p.read_bytes() for p in trash.rglob('*') if p.is_file()}
                page.locator('.trash-item [data-act=purge]').first.click();confirm(page,False)
                assert before=={p.relative_to(trash):p.read_bytes() for p in trash.rglob('*') if p.is_file()}
                page.locator('.trash-item [data-act=purge]').first.click();confirm(page,True)
                expect(page.locator('.trash-item')).to_have_count(2);assert len(list(trash.iterdir()))==2
                page.locator('#trash-purge-all').click();confirm(page,False)
                assert len(list(trash.iterdir()))==2
                page.locator('#trash-purge-all').click();confirm(page,True)
                expect(page.locator('#trash-list')).to_contain_text('没有已删除的会话')
                assert not list(trash.iterdir())
                page.screenshot(path=str(args.output/'empty-narrow.png'))
                page.reload(wait_until='networkidle');open_trash(page)
                expect(page.locator('#trash-list')).to_contain_text('没有已删除的会话')
                assert all(not p.exists() for p in data.paths.values())
                (args.output/'result.json').write_text(json.dumps({'passed':['actual confirmation clicks/cancel','delete receipt entry','restore conflict/retry/disk bytes','refresh','wide/narrow close/backdrop/Escape/Tab Enter','purge and clear all/cancel/private ledger','refresh preserves empty bin']},indent=2))
                print('PASS trash actual mouse/keyboard inputs, conflict retry, cancellation and private native/ledger persistence',flush=True)
            finally:browser.close()


if __name__ == '__main__':
    from browser_evidence_fixtures import run_with_evidence
    run_with_evidence(main, 'target/trash-interactions')
