"""Browser actions shared by SDK presentation suites."""
# run_validation: skip
from playwright.sync_api import expect


def start_sdk(page):
    """Finish opening history, then explicitly start an inactive SDK view."""
    expect(page.locator('.sdk-pane')).to_be_visible()
    page.wait_for_function('SdkView.pane && !SdkView.historyReady')
    if page.evaluate('SdkView.readOnly'):
        button = page.locator('#a-session-toggle')
        if not button.is_visible():
            page.locator('#a-more').click()
        button.click()
    expect(page.locator('#sdk-input')).to_be_visible()
    page.wait_for_function('SdkView.connected')
