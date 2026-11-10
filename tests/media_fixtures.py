"""Shared browser interactions for native-media suites."""

from playwright.sync_api import Page, expect


def expand_media_tools(page: Page) -> None:
    """Open the actual process and tool-group disclosures before inspecting images."""
    for selector in (
        "#msgs .turn-process.folded > .turn-toolbar .fold-toggle",
        "#msgs .grp.folded > .group-preview > .fold-toggle",
    ):
        toggles = page.locator(selector)
        while count := toggles.count():
            toggles.first.click()
            expect(toggles).to_have_count(count - 1)
