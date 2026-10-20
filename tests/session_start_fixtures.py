"""Launch settings exercised through their visible controls."""
# run_validation: skip
def choose_mode(page, source, mode):
    if not page.locator('#settings').is_visible(): page.locator('#header-more-btn').click()
    page.locator('#settings').click()
    page.locator('.settings-tab[data-tab="features"]').click()
    page.locator(f'[data-agent-start-mode="{source}"]').select_option(mode)
