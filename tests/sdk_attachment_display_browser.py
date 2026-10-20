#!/usr/bin/env python3
"""Published attachment previews across the four native SDKs and ordinary chat.

Real Chromium uploads and sends to private synthetic CLIs. Codex node/Hub live
and history coverage lives in codex_sdk_compose_browser.py.
"""
import argparse
from pathlib import Path
import tempfile
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright
from codex_sdk_compose_browser import attach, saved
from history_fixtures import BINARY
from sdk_start_stop_fixtures import provider


def check(page, selector):
    bubble = page.locator(selector).filter(has_text='附件展示回归').last
    image = bubble.locator('.media-link img')
    expect(image).to_have_count(1)
    image.hover()
    expand = bubble.get_by_text('展开全文', exact=False)
    if expand.is_visible():
        expand.click()
    expect(image).to_be_visible()
    expect(image).to_have_js_property('complete', True)
    expect(image).to_have_js_property('naturalWidth', 1280)
    expect(bubble.locator('a[data-file-ref]').filter(has_text='附件1: 屏幕截图.png')).to_have_count(1)
    document = bubble.locator('a[data-file-ref]').filter(has_text='项目 notes.txt')
    expect(document).to_have_count(1)
    for link, name in [(bubble.locator('.media-link'), '屏幕截图.png'),
                       (document, '项目 notes.txt')]:
        with page.context.expect_page() as opened:
            link.click()
        preview = opened.value
        preview.wait_for_load_state('domcontentloaded')
        if name.endswith('.png') and '/api/media/' in urlsplit(preview.url).path:
            expect(preview.locator('img')).to_have_js_property('naturalWidth', 1280)
            preview.close()
            continue
        expect(preview.locator('#file-title')).to_contain_text(name)
        if name.endswith('.txt'):
            expect(preview.locator('#file-content pre')).to_have_text('attachment document\n')
        else:
            expect(preview.locator('iframe[title="文件预览"]')).to_be_visible()
        with preview.expect_download() as event:
            preview.locator('#file-download').click()
        assert event.value.failure() is None
        assert event.value.suggested_filename == name
        preview.close()


def exercise(browser, base, source, screenshots):
    context = browser.new_context(viewport={'width': 1280, 'height': 900},
                                  color_scheme='light', service_workers='block')
    context.route('**/*', lambda route: route.continue_()
                  if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    page.set_default_timeout(15000)
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    try:
        page.goto(base, wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        row = page.locator(f'#side .item[data-uid^="{source}:"]').first
        if not row.is_visible():
            page.locator(f'#chips button[data-source="{source}"]').click()
        row.click()
        page.locator('#a-sdk').click()
        page.locator('#a-session-toggle').click()
        expect(page.locator('#sdk-input')).to_be_enabled()
        uid = page.evaluate('S.sel')
        page.locator('#sdk-input').fill('附件展示回归：请查看图片和项目文件。')
        image_data = page.screenshot()
        attach(page, 'image', '屏幕截图.png', 'image/png', image_data)
        attach(page, 'file', '项目 notes.txt', 'text/plain', b'attachment document\n')
        saved(page, uid, '附件展示回归：请查看图片和项目文件。')
        page.locator('#sdk-send').click()
        expect(page.locator('#sdk-compose-items .draft-card')).to_have_count(0)
        expect(page.locator('#sdk-stop')).to_be_hidden()
        page.wait_for_function("() => NativeSdkView.historyData?.messages.some(message => message.role === 'user' && message.text.includes('附件展示回归'))")
        for width, theme in ((1280, 'light'), (390, 'dark'), (1280, 'dark'), (390, 'light')):
            print(f'{source}: sent attachments {width} {theme}', flush=True)
            page.set_viewport_size({'width': width, 'height': 900})
            page.emulate_media(color_scheme=theme)
            page.evaluate('() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))')
            if width < 600 and not page.evaluate("document.body.classList.contains('mobile-detail')"):
                page.locator(f'#side .item[data-uid="{uid}"]').click()
            check(page, '.native-sdk-pane .msg[data-role="user"]')
            page.screenshot(path=str(screenshots/f'{source}-{width}-{theme}-sdk.png'))
            page.reload(wait_until='networkidle')
            if width < 600 and not page.evaluate("document.body.classList.contains('mobile-detail')"):
                page.locator(f'#side .item[data-uid="{uid}"]').click()
            check(page, '.native-sdk-pane .msg[data-role="user"]')
            page.locator('#a-chat').click()
            check(page, '#msgs .msg[data-role="user"]')
            page.screenshot(path=str(screenshots/f'{source}-{width}-{theme}-chat.png'))
            page.locator('#a-sdk').click()
        assert not errors, errors
    except BaseException:
        print('attachment diagnostic:', page.evaluate("""() => ({uid:S.sel,
          links:[...document.querySelectorAll('.native-sdk-pane .media-link')].map(link=>link.outerHTML),
          menu:document.querySelector('#file-menu')?.innerText,
          history:NativeSdkView.historyData?.messages.filter(message=>message.text.includes('附件展示回归'))})"""), flush=True)
        for image in page.locator('.native-sdk-pane .media-link img').all():
            response = page.request.get(image.get_attribute('src'))
            print('attachment response:', response.status, response.text() if not response.ok else response.headers.get('content-type'), flush=True)
        page.screenshot(path=str(screenshots/f'{source}-failure.png'))
        raise
    finally:
        context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, required=True)
    parser.add_argument('--sources', nargs='+', default=['claude', 'grok', 'agy', 'opencode'])
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-attachment-display-') as tmp, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in args.sources:
                root = Path(tmp)/source
                root.mkdir()
                with provider(source, root, args.binary) as (base, _, _):
                    exercise(browser, base, source, args.screenshots)
        finally:
            browser.close()
    print('PASS four SDK attachment previews: upload/SEND, reload/chat, downloads, wide/narrow light/dark')


if __name__ == '__main__':
    main()
