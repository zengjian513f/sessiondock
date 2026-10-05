#!/usr/bin/env python3
"""Chromium user acceptance: the appearance/features settings panes.

Runs against the served frontend (legacy-web/ by default, which needs no
build). This script only drives the served UI.

Synthetic corpus and a loopback isolated_server. Desktop 1280×900 and phone
390×844 open the existing settings button (the phone header-more menu when
the gear is folded), click appearance/features/machines tabs, and change the
existing theme, font, scale, cache, sleep, stop-concurrency and paste-files
controls. Close, reopen and reload must show the same selections. Escape
closes the dialog. The machines tab is only opened and left. Synthetic
beforeinstallprompt events enable real install-button clicks; deferred prompt
choices cover dismissed/accepted, and appinstalled updates the same button.
The prompt receiver must remain the original event.

The last settings tab is restored on open, so every check clicks its tab.
Scale coverage here is a short keyboard nudge plus reset, not the matrix in
prefs_migration_browser.py. No real CLI and no production sessions.
"""
import argparse
import os
import tempfile
from pathlib import Path

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, build_corpus, isolated_server

VIEWPORTS = ((1280, 900), (390, 844))
TABS = {"appearance": "外观", "features": "功能", "machines": "机器"}
SUBTITLES = {
    "appearance": "界面偏好保存在浏览器",
    "features": "功能偏好保存在此浏览器",
    "machines": "机器设置保存在中央服务端，所有浏览器一致",
}
INSTALL_TITLE = "浏览器尚未提供安装能力"


def launch_chromium(playwright):
    options = {"headless": True}
    executable = os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE")
    if executable:
        options["executable_path"] = executable
    return playwright.chromium.launch(**options)


def wait_app(page):
    expect(page.locator("#side .item[data-uid]").first).to_be_visible()
    expect(page.locator("#backend-notice")).to_be_hidden()


def open_settings(page):
    settings = page.locator("#settings")
    more = page.locator("#header-more-btn")
    expect(page.locator('#settings:visible, #header-more-btn:visible').first).to_be_visible()
    if not settings.is_visible():
        more.click()
        expect(settings).to_be_visible()
    settings.click()
    expect(page.locator("#settings-dialog")).to_be_visible()
    expect(page.locator("#settings-title")).to_have_text("设置")


def select_tab(page, name):
    page.get_by_role("tab", name=TABS[name], exact=True).click()
    expect(page.locator(f'.settings-tab[data-tab="{name}"]')).to_have_attribute("aria-selected", "true")
    expect(page.locator(f"#settings-{name}")).to_be_visible()
    expect(page.locator("#settings-sub")).to_have_text(SUBTITLES[name])
    for other in TABS:
        if other == name:
            continue
        expect(page.locator(f"#settings-{other}")).to_be_hidden()
        expect(page.locator(f'.settings-tab[data-tab="{other}"]')).to_have_attribute("aria-selected", "false")


def expect_option(control, value, label):
    expect(control).to_have_value(value)
    expect(control.locator("option:checked")).to_have_text(label)


def expect_install(page):
    button = page.locator("[data-pwa-install-button]")
    button.scroll_into_view_if_needed()
    expect(button).to_be_visible()
    expect(button).to_be_disabled()
    expect(button).to_have_text("安装到桌面")
    expect(button).to_have_attribute("title", INSTALL_TITLE)


def dispatch_install_prompt(page):
    assert page.evaluate("""() => {
      const event = new Event('beforeinstallprompt', {cancelable: true});
      const probe = {calls: 0, original: false, resolve: null};
      event.prompt = function() {
        probe.calls++;
        probe.original = this === event;
        return new Promise(resolve => {probe.resolve = resolve;});
      };
      window.__pwaInstallProbe = probe;
      window.dispatchEvent(event);
      return event.defaultPrevented;
    }"""), "beforeinstallprompt must suppress the browser's default prompt"
    button = page.locator("[data-pwa-install-button]")
    expect(button).to_be_enabled()
    expect(button).to_have_text("安装到桌面")
    expect(button).to_have_attribute("title", "安装为独立桌面应用")
    return button


def expect_installed(page):
    button = page.locator("[data-pwa-install-button]")
    expect(button).to_be_visible()
    expect(button).to_be_disabled()
    expect(button).to_have_text("已安装")
    expect(button).to_have_attribute("title", "当前已作为独立应用运行")


def exercise_install(page):
    open_settings(page)
    select_tab(page, "appearance")
    expect_install(page)

    for outcome in ("dismissed", "accepted"):
        button = dispatch_install_prompt(page)
        button.click()
        # The event is consumed before the asynchronous choice finishes.
        expect_install(page)
        assert page.evaluate("""() => ({
          calls: window.__pwaInstallProbe.calls,
          original: window.__pwaInstallProbe.original,
        })""") == {"calls": 1, "original": True}
        page.evaluate("""outcome => window.__pwaInstallProbe.resolve({outcome})""", outcome)
        if outcome == "dismissed":
            expect_install(page)
        else:
            expect_installed(page)

    # Closing and reopening settings keeps the accepted install state.
    page.keyboard.press("Escape")
    expect(page.locator("#settings-dialog")).to_be_hidden()
    open_settings(page)
    select_tab(page, "appearance")
    expect_installed(page)

    # A new page lets appinstalled exercise its own transition independently.
    page.reload(wait_until="domcontentloaded")
    wait_app(page)
    open_settings(page)
    select_tab(page, "appearance")
    expect_install(page)
    dispatch_install_prompt(page)
    page.evaluate("() => window.dispatchEvent(new Event('appinstalled'))")
    expect_installed(page)
    assert page.evaluate("() => window.__pwaInstallProbe.calls") == 0
    page.keyboard.press("Escape")
    expect(page.locator("#settings-dialog")).to_be_hidden()


def nudge_scale(page, steps):
    slider = page.locator("#setting-scale")
    output = page.locator("#setting-scale-value")
    start = int(slider.input_value())
    slider.focus()
    for _ in range(steps):
        slider.press("ArrowRight")
    value = str(start + steps)
    expect(slider).to_have_value(value)
    expect(output).to_have_text(f"{value}%")


def expect_saved(page):
    select_tab(page, "appearance")
    expect_option(page.locator("#setting-theme"), "dark", "深色")
    expect(page.locator("html")).to_have_attribute("data-theme", "dark")
    expect_option(page.locator("#setting-font"), "cascadia", "Cascadia Mono（内置）")
    expect(page.locator("#setting-scale")).to_have_value("105")
    expect(page.locator("#setting-scale-value")).to_have_text("105%")
    expect_install(page)
    select_tab(page, "features")
    expect_option(page.locator("#setting-cache"), "128", "128 MB")
    expect_option(page.locator("#setting-sleep"), "15", "15 分钟")
    expect_option(page.locator("#setting-stop-concurrency"), "2", "2 个")
    expect(page.locator("#setting-console-paste-files")).to_be_checked()


def exercise(browser, base, width, height):
    errors = []
    context = browser.new_context(
        viewport={"width": width, "height": height},
        service_workers="block",
        color_scheme="light",
    )
    context.route("**/*", lambda route: route.continue_()
                  if route.request.url.startswith(base + "/") else route.abort())
    page = context.new_page()
    page.on("pageerror", lambda error: errors.append(str(error)))
    try:
        page.goto(base, wait_until="domcontentloaded")
        wait_app(page)
        open_settings(page)

        select_tab(page, "appearance")
        expect_option(page.locator("#setting-theme"), "system", "跟随系统")
        expect(page.locator("html")).to_have_attribute("data-theme", "light")
        expect_option(page.locator("#setting-font"), "ubuntu", "Ubuntu Sans Mono（26.04 默认）")
        expect(page.locator("#setting-scale")).to_have_value("100")
        expect(page.locator("#setting-scale-value")).to_have_text("100%")
        expect_install(page)

        theme = page.locator("#setting-theme")
        theme.select_option("light")
        expect_option(theme, "light", "浅色")
        expect(page.locator("html")).to_have_attribute("data-theme", "light")
        theme.select_option("dark")
        expect_option(theme, "dark", "深色")
        expect(page.locator("html")).to_have_attribute("data-theme", "dark")
        page.locator("#setting-font").select_option("cascadia")
        expect_option(page.locator("#setting-font"), "cascadia", "Cascadia Mono（内置）")

        nudge_scale(page, 5)
        reset = page.locator("#setting-scale-reset")
        expect(reset).to_have_text("重置")
        reset.click()
        expect(page.locator("#setting-scale")).to_have_value("100")
        expect(page.locator("#setting-scale-value")).to_have_text("100%")
        nudge_scale(page, 5)

        select_tab(page, "features")
        expect_option(page.locator("#setting-cache"), "256", "256 MB")
        expect_option(page.locator("#setting-sleep"), "60", "1 小时（默认）")
        expect_option(page.locator("#setting-stop-concurrency"), "6", "6 个（默认）")
        paste = page.locator("#setting-console-paste-files")
        expect(paste).not_to_be_checked()
        page.locator("#setting-cache").select_option("128")
        page.locator("#setting-sleep").select_option("15")
        page.locator("#setting-stop-concurrency").select_option("2")
        paste.click()
        expect(paste).to_be_checked()

        select_tab(page, "machines")
        expect(page.locator("#machine-rows")).not_to_be_empty()
        expect(page.locator("#settings-machines")).to_contain_text("AI 客户端")
        select_tab(page, "appearance")
        expect_option(page.locator("#setting-theme"), "dark", "深色")

        page.locator("#settings-dialog .modal-close").click()
        expect(page.locator("#settings-dialog")).to_be_hidden()
        open_settings(page)
        expect_saved(page)
        page.keyboard.press("Escape")
        expect(page.locator("#settings-dialog")).to_be_hidden()

        page.reload(wait_until="domcontentloaded")
        wait_app(page)
        expect(page.locator("html")).to_have_attribute("data-theme", "dark")
        open_settings(page)
        expect_saved(page)
        page.keyboard.press("Escape")
        expect(page.locator("#settings-dialog")).to_be_hidden()
        exercise_install(page)
        assert not errors, errors
        print(f"PASS {width}x{height}: settings tabs, theme/font, scale, features, machines, persisted, PWA install", flush=True)
    finally:
        context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-frontend-framework-") as temporary:
        corpus = build_corpus(Path(temporary))
        with isolated_server(corpus, args.binary) as (base, _), sync_playwright() as playwright:
            browser = launch_chromium(playwright)
            try:
                for width, height in VIEWPORTS:
                    exercise(browser, base, width, height)
            finally:
                browser.close()
    print("PASS frontend_framework_browser", flush=True)


if __name__ == "__main__":
    main()
