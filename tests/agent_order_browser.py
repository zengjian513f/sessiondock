#!/usr/bin/env python3
"""Agent order across node/Hub UI and default Codex creation with a fake SDK."""
import argparse
import tempfile
from pathlib import Path
from types import SimpleNamespace
from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY
from sdk_start_stop_fixtures import provider
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from new_session_model_browser import open_dialog
from bug_report_node_browser import open_report
from frontend_framework_browser import open_settings
from browser_runtime import open_toolbar_options

ORDER = ['codex', 'kiro', 'agy', 'grok', 'claude', 'opencode']
LABELS = ['Codex', 'Kiro', 'Agy', 'Grok', 'Claude', 'OpenCode']


def values(page, selector, attribute):
    return page.locator(selector).evaluate_all('(els, key) => els.map(e => e.getAttribute(key))', attribute)


def exercise(browser, base, root, output, label, width):
    context = browser.new_context(viewport={'width': width, 'height': 950},
                                  color_scheme='dark' if width < 720 else 'light', service_workers='block')
    page = context.new_page()
    errors = []
    page.on('pageerror', lambda e: errors.append(str(e)))
    # Deliberately unordered installed clients, as returned by independent probes.
    clients = [{'source': s, 'installed': True, 'version': '1.0', 'latest': '1.0'} for s in reversed(ORDER)]
    page.route('**/api/clients', lambda r: r.fulfill(json={'clients': clients}))
    page.route('**/api/quotas*', lambda r: r.fulfill(json={'agents': []}))
    def shot(name):
        page.screenshot(path=str(output / f'{label}-{width}-{name}.png'), full_page=True)
    page.goto(base, wait_until='networkidle')
    menu = open_toolbar_options(page, 'sources')
    assert values(page, '#toolbar-sources-menu button[data-source]', 'data-source') == ORDER + ['shell']
    shot('filters')
    page.keyboard.press('Escape')
    page.keyboard.press('Escape')
    open_dialog(page)
    assert values(page, 'input[name=new-source]', 'value') == ORDER + ['shell']
    expect(page.locator('input[name=new-source][value=codex]')).to_be_checked()
    shot('new')
    page.locator('#new-cwd').fill(str(root / 'report-work'))
    with page.expect_response(lambda r: r.url.endswith('/codex-sdk/create') and r.request.method == 'POST') as created:
        page.locator('#new-session-go').click()
    assert created.value.ok, created.value.text()
    assert created.value.request.post_data_json['source'] == 'codex'
    expect(page.locator('#sdk-input')).to_be_visible()
    # Leave detail on narrow displays so header menus are available.
    page.reload(wait_until='networkidle')
    if width < 720 and page.locator('.mobile-back').is_visible():
        page.locator('.mobile-back').click()
    open_report(page)
    assert values(page, 'input[name=bug-report-source]', 'value') == ORDER
    expect(page.locator('input[name=bug-report-source][value=codex]')).to_be_checked()
    shot('report')
    page.locator('#bug-report-dialog .modal-close').click()
    open_settings(page)
    page.locator('.settings-tab[data-tab=features]').click()
    assert values(page, '[data-agent-start-mode]', 'data-agent-start-mode') == ORDER
    # Persist a user choice and verify it after reload.
    page.locator('[data-agent-start-mode=codex]').select_option('cli')
    shot('settings')
    page.locator('.settings-tab[data-tab=machines]').click()
    expect(page.locator('#client-matrix thead th')).to_have_text(['机器'] + LABELS)
    shot('clients')
    page.get_by_role('link', name='Agent 额度 →').click()
    expect(page.locator('#quota-dialog .quota-agent')).to_have_count(4)
    assert values(page, '#quota-dialog .quota-agent', 'data-source') == ['codex', 'kiro', 'agy', 'claude']
    shot('quotas-dialog')
    page.reload(wait_until='networkidle')
    if width < 720 and page.locator('.mobile-back').is_visible():
        page.locator('.mobile-back').click()
    open_settings(page)
    page.locator('.settings-tab[data-tab=features]').click()
    expect(page.locator('[data-agent-start-mode=codex]')).to_have_value('cli')
    page.goto(base + '/quotas.html', wait_until='networkidle')
    expect(page.locator('.quota-agent')).to_have_count(4)
    assert values(page, '.quota-agent', 'data-source') == ['codex', 'kiro', 'agy', 'claude']
    shot('quotas-page')
    assert not errors, errors
    context.close()
    print(f'PASS {label} {width}: ordered filters/create/report/settings/clients/quotas, default SDK creation, preference persistence', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--output', type=Path, default=Path('/tmp/sessiondock-agent-order'))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-agent-order-') as tmp, sync_playwright() as pw:
        root = Path(tmp)
        with provider('codex', root, args.binary, report=True) as (base, _, extra):
            hubroot = root / 'hub'; hubroot.mkdir()
            node = SimpleNamespace(name='Agent order node', token=TOKEN, port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))
            hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot, [node])
            hub.start()
            browser = pw.chromium.launch(headless=True)
            try:
                for label, url in [('node', base), ('hub', f'http://127.0.0.1:{hub.port}')]:
                    for width in (1400, 390):
                        exercise(browser, url, root, args.output, label, width)
            finally:
                browser.close()
                hub.stop()


if __name__ == '__main__':
    main()
