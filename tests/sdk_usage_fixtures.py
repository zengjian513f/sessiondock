"""Browser interactions for always-visible SDK usage and optional details."""
import re
import time
from playwright.sync_api import expect


def assert_reset_countdown(page, node, end):
    """Exercise local time updates on an already opened user-facing surface."""
    for remaining, label in ((3*86400+3*3600+38*60, '3d 03:38'),
                             (3*3600+38*60, '03:38'), (86400, '1d 00:00'),
                             (59, '00:00'), (0, '已到期'), (-60, '已到期')):
        page.clock.set_fixed_time(end-remaining)
        expect(node).to_have_attribute('title', re.compile('重置剩余 ' + re.escape(label)))
        if node.get_attribute('aria-description') is not None:
            expect(node).to_have_attribute('aria-description', re.compile('重置剩余 ' + re.escape(label)))
    page.clock.set_fixed_time(time.time())


def assert_inline_usage(page):
    gauges = page.locator('.sdk-pane .sdk-status-row > .sdk-gauges')
    expect(gauges).to_be_visible()
    expect(page.locator('.sdk-pane .sdk-status-row > #sdk-usage-toggle')).to_be_hidden()
    assert_single_status_row(page.locator('.sdk-pane .sdk-status-row'))


def assert_single_status_row(row):
    geometry = row.evaluate("""row => {
      const bounds = row.getBoundingClientRect();
      const visible = [...row.children].filter(n => !n.hasAttribute('popover') && n.getClientRects().length);
      return {overflow:row.scrollWidth-row.clientWidth, top:bounds.top, bottom:bounds.bottom,
        children:visible.map(n => {const r=n.getBoundingClientRect();return {top:r.top,bottom:r.bottom,left:r.left,right:r.right};}),
        status:row.querySelector('.sdk-status').textContent};
    }""")
    assert geometry['overflow'] <= 1, geometry
    assert len(geometry['status']) <= 6, geometry
    for child in geometry['children']:
        assert child['top'] >= geometry['top'] - 1 and child['bottom'] <= geometry['bottom'] + 1, geometry
    gauges = row.locator(':scope > .sdk-gauges')
    assert gauges.bounding_box()['width'] > 0
    if gauges.evaluate('e => e.scrollWidth <= e.clientWidth + 1'):
        alignment = gauges.evaluate('''e => {
          const last = [...e.children].filter(n => !n.hidden).at(-1);
          return last ? e.getBoundingClientRect().right - last.getBoundingClientRect().right : 0;
        }''')
        assert abs(alignment) <= 2, ('usage must align right', alignment)
    model = row.locator('.sdk-pickers > .sdk-model')
    if model.count():
        assert gauges.bounding_box()['x'] + gauges.bounding_box()['width'] <= model.bounding_box()['x'] + 1, 'usage must stay left of model'
    # Large quota sets scroll inside their single line; controls yield first.
    if gauges.evaluate('e => e.scrollWidth > e.clientWidth + 1'):
        assert row.locator('.sdk-pickers > label').count() == 0


def toggle_usage_details(page):
    menu = page.locator('#sdk-usage-menu')
    if menu.is_visible():
        page.keyboard.press('Escape')
        expect(menu).to_be_hidden()
        return
    if page.locator('.sdk-gauges > :not([hidden])').count():
        assert_inline_usage(page)
    if not page.locator('#sdk-more-menu').is_visible():
        page.locator('#sdk-more').click()
    if menu.get_attribute('popover') is not None:
        page.get_by_role('button', name='用量明细', exact=True).click()
    expect(menu).to_be_visible()


def refresh_usage(page):
    """Use the visible composer; refreshing must not wait before clearing input."""
    page.keyboard.press('Escape')
    page.locator('#sdk-input').fill('/usage')
    page.locator('#sdk-send').click()
    expect(page.locator('#sdk-input')).to_have_value('')
