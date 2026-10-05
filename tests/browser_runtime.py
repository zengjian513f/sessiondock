"""Choose explicit legacy/scoped browser scripts against the real app graph.

Fixture source supplies both complete scripts. This helper never rewrites JS,
installs globals, copies service state, or manufactures operations.
"""
import time

from frontend_paths import entry_asset, frontend_dir, frontend_html


def scoped_frontend() -> bool:
    """Select scoped scripts from the served HTML, including the default build."""
    entry_asset()  # Require a real built entry before choosing fixture scripts.
    return bool(frontend_html().modules)


def console_renderers(*renderers: str) -> list[str]:
    """Console renderers this build serves: the Vue build has only the grid."""
    return [name for name in renderers if name != 'xterm'] if scoped_frontend() else list(renderers)


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
