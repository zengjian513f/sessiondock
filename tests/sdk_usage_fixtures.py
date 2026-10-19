"""Browser interactions for always-visible SDK usage and optional details."""
from playwright.sync_api import expect


def assert_inline_usage(page):
    gauges = page.locator('.sdk-pane .sdk-status-row > .sdk-gauges')
    expect(gauges).to_be_visible()
    expect(page.locator('.sdk-pane .sdk-status-row > #sdk-usage-toggle')).to_be_hidden()
    measurements = gauges.evaluate("""element => {
      const row = element.parentElement, bounds = row.getBoundingClientRect();
      return {left: bounds.left, right: bounds.right, overflow: row.scrollWidth - row.clientWidth,
        children: [...element.children].filter(node => !node.hidden).map(node => {
          const box = node.getBoundingClientRect(); return {left: box.left, right: box.right};
        })};
    }""")
    assert measurements['overflow'] <= 1, measurements
    assert measurements['children'], measurements
    for child in measurements['children']:
        assert child['left'] >= measurements['left'] - 1, measurements
        assert child['right'] <= measurements['right'] + 1, measurements


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
