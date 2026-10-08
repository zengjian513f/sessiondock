"""Browser helpers shared by the Chromium suites."""
import time

from frontend_paths import entry_asset, frontend_dir


def entry_pattern() -> str:
    """Playwright pattern for that exact entry, including its query string."""
    asset = entry_asset()
    return '**/' + asset.relative_to(frontend_dir()).as_posix().removeprefix('./') + '*'


def wait_for_async(page, script: str, *, arg=None, timeout=None, polling="raf"):
    """Wait for the resolved predicate value, rather than a truthy Promise.

    Use the same evaluation coroutine and sync dispatcher as Page.evaluate.
    The public evaluate API has no timeout, so asyncio.wait_for bounds each
    actual evaluation by the remaining original deadline without changing JS.
    """
    import asyncio
    from playwright.sync_api import TimeoutError as PlaywrightTimeoutError

    timeout_ms = page._impl_obj._timeout_settings.timeout(timeout)
    deadline = time.monotonic() + timeout_ms / 1000 if timeout_ms else None
    if polling != "raf" and (not isinstance(polling, (int, float)) or polling <= 0):
        raise ValueError("polling must be 'raf' or a positive millisecond interval")

    def remaining():
        if deadline is None:
            return None
        seconds = deadline - time.monotonic()
        if seconds <= 0:
            raise PlaywrightTimeoutError(f"wait_for_async: Timeout {timeout_ms}ms exceeded")
        return seconds

    def await_operation(operation):
        try:
            limit = remaining()
        except PlaywrightTimeoutError:
            operation.close()
            raise
        try:
            return page._sync(asyncio.wait_for(operation, timeout=limit))
        except asyncio.TimeoutError as error:
            raise PlaywrightTimeoutError(f"wait_for_async: Timeout {timeout_ms}ms exceeded") from error

    while True:
        remaining()
        value = await_operation(page._impl_obj.evaluate(script, arg))
        if value:
            return value
        if polling == "raf":
            await_operation(page._impl_obj.evaluate("() => new Promise(requestAnimationFrame)"))
        else:
            left = remaining()
            interval = polling if left is None else min(polling, left * 1000)
            await_operation(page._impl_obj.wait_for_timeout(interval))


def open_toolbar_options(page, kind):
    """Open the toolbar's second-level options through user clicks."""
    if not page.locator('#header-menu').is_visible():
        page.locator('#header-more-btn').click()
    page.locator(f'#header-menu [data-toolbar-menu="{kind}"]').click()
    return page.locator(f'#toolbar-{kind}-menu')


def choose_toolbar_option(page, kind, value):
    """Choose a pinned button or its equivalent under the header ellipsis."""
    host, attribute = {'view': ('view', 'v'), 'nest': ('nest', 'mode'),
                       'sources': ('chips', 'source')}[kind]
    selector = f'button[data-{attribute}="{value}"]'
    inline = page.locator(f'#{host} {selector}')
    if inline.is_visible():
        inline.click()
    else:
        open_toolbar_options(page, kind).locator(selector).click()
