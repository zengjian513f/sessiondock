"""Choose explicit legacy/scoped browser scripts against the real app graph.

Fixture source supplies both complete scripts. This helper never rewrites JS,
installs globals, copies service state, or manufactures operations.
"""
import os
import time
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlsplit

from frontend_paths import frontend_dir


def scoped_frontend() -> bool:
    """Select the isolated artifact using the existing test-only source selector."""
    selected = os.environ.get("SESSIONDOCK_TEST_WEB_DIR")
    # Both entries can be built in private snapshots. Select fixture scripts
    # from the actual served entry, rather than treating every custom path as Vue.
    return bool(selected and entry_asset(Path(selected)).name != 'app.js')


def init_js(legacy: str, scoped: str) -> str:
    """Choose explicit startup scripts; their callbacks resolve the graph later."""
    return scoped if scoped_frontend() else legacy


def js(legacy: str, scoped: str, *, body: bool = False) -> str:
    isolated_artifact = scoped_frontend()
    # New entry startup predicates must wait for the actual composition graph.
    # No missing graph is substituted with legacy product state.
    fallback = "false" if isolated_artifact else f"({legacy})"
    if body:
        legacy_body = '' if isolated_artifact else legacy
        return (
            "() => { const runtime = window.SessionDockRuntime; "
            f"if (runtime) {{ {scoped} }} else {{ {legacy_body} }} }}"
        )
    return (
        "(() => { const runtime = window.SessionDockRuntime; "
        f"return runtime ? ({scoped}) : {fallback}; }})()"
    )


def entry_asset(directory: Path | None = None) -> Path:
    """Find the actual served main entry in a private frontend artifact."""
    directory = Path(directory or frontend_dir()).resolve()
    class Scripts(HTMLParser):
        def __init__(self):
            super().__init__()
            self.modules = []
            self.classic = []

        def handle_starttag(self, tag, attrs):
            values = dict(attrs)
            if tag == 'script' and values.get('src'):
                target = self.modules if values.get('type') == 'module' else self.classic
                target.append(values['src'])

    parser = Scripts()
    parser.feed((directory / 'index.html').read_text())
    if parser.modules:
        assert len(parser.modules) == 1, parser.modules
        src = parser.modules[0]
    else:
        candidates = [src for src in parser.classic if Path(urlsplit(src).path).name == 'app.js']
        assert len(candidates) == 1, parser.classic
        src = candidates[0]
    url = urlsplit(src)
    assert not url.scheme and not url.netloc, f'external entry: {src}'
    entry = (directory / unquote(url.path).lstrip('/')).resolve()
    assert entry.is_relative_to(directory) and entry.is_file(), f'missing local entry: {src}'
    return entry


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
