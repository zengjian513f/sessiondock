"""Browser actions shared by SDK presentation suites."""
# run_validation: skip
from playwright.sync_api import expect


def reveal_sdk_tool(tool):
    """Open the completed process and tool group through their real controls."""
    for name in ('turn-process', 'sdk-tool-group'):
        if tool.is_visible():
            return
        parent = tool.locator(f"xpath=ancestor::*[contains(concat(' ',normalize-space(@class),' '),' {name} ')][1]")
        if parent.count():
            parent.locator('.fold-toggle').first.click()
    expect(tool).to_be_visible()


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
