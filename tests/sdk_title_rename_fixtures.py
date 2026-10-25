"""Shared user interactions for the SDK heading rename entry."""
from playwright.sync_api import expect


def exercise_title_rename(page, title, screenshots=None, label='sdk', layouts=None):
    box = page.locator('#sdk-input')
    heading = page.locator('#a-session-title')
    dialog = page.get_by_role('dialog', name='重命名会话', exact=True)
    # A command-shaped draft is still a draft when the heading opens the form.
    box.fill('/rename')
    for width, theme in layouts or [(1280, 'light'), (390, 'dark'), (1280, 'dark'), (390, 'light')]:
        page.set_viewport_size({'width': width, 'height': 900})
        page.emulate_media(color_scheme=theme)
        page.evaluate('() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))')
        if width < 500 and not page.locator('body').evaluate("body => body.classList.contains('mobile-detail')"):
            page.locator('#side .item[data-uid="' + page.evaluate('S.sel') + '"]').click()
        expect(heading).to_be_visible()
        expect(heading).to_have_attribute('aria-label', '重命名会话')
        heading.click()
        expect(dialog.get_by_role('textbox', name='会话名称')).to_have_value(title)
        if screenshots:
            page.screenshot(path=str(screenshots / f'{label}-{width}-{theme}-title-dialog.png'), full_page=True)
        dialog.get_by_role('button', name='取消', exact=True).click()
        expect(box).to_have_value('/rename')
        expect(heading).to_be_enabled()
        heading.focus()
        page.keyboard.press('Enter')
        field = dialog.get_by_role('textbox', name='会话名称')
        field.fill('  ' + title + ' · 标题编辑  ')
        dialog.get_by_role('button', name='保存', exact=True).click()
        expect(dialog).to_be_hidden()
        expect(heading).to_be_enabled()
        expect(heading).to_have_text(title + ' · 标题编辑')
        page.wait_for_function('title => S.sessions.some(row => row.uid === S.sel && row.title === title)', arg=title + ' · 标题编辑')
        expect(box).to_have_value('/rename')
        if screenshots:
            page.screenshot(path=str(screenshots / f'{label}-{width}-{theme}-title-saved.png'), full_page=True)
        heading.click()
        dialog.get_by_role('textbox', name='会话名称').fill(title)
        dialog.get_by_role('button', name='保存', exact=True).click()
        expect(heading).to_be_enabled()
        expect(heading).to_have_text(title)
        expect(box).to_have_value('/rename')
    box.fill('')
