"""Answer SessionDock's in-page popups the way tests answered native dialogs.

`appAlert`/`appConfirm` open a centered modal `<dialog class="app-popup">` instead of
`alert`/`confirm`. `on_popup(page_or_context, handler)` gives `handler` an object shaped
like Playwright's `Dialog` (`type`, `message`, `accept()`, `dismiss()`) for every popup
that opens, then clicks the matching button in the page: 确定/知道了 for accept,
取消 for dismiss (the default when no handler answers). Registering on a context
covers every page of it; `once=True` answers a single popup.
"""
from __future__ import annotations

import traceback

WATCH = """() => {
  if (window.__sdPopupWatch) return;
  window.__sdPopupWatch = true;
  const seen = new WeakSet();
  const scan = () => {
    for (const dialog of document.querySelectorAll('dialog.app-popup[open]')) {
      if (seen.has(dialog)) continue;
      seen.add(dialog);
      window.__sdPopup(dialog.dataset.popupType || 'alert', dialog.dataset.message || '').then(answer => {
        const button = dialog.querySelector(`[data-popup-action="${answer}"]`)
          || dialog.querySelector('[data-popup-action="ok"]');
        if (dialog.open) button?.click();
      });
    }
  };
  const start = () => {
    new MutationObserver(scan).observe(document.documentElement,
      {childList: true, subtree: true, attributes: true, attributeFilter: ['open']});
    scan();
  };
  if (document.documentElement) start();
  else document.addEventListener('DOMContentLoaded', start, {once: true});
}"""


class Popup:
    def __init__(self, kind: str, message: str):
        self.type = kind
        self.message = message
        self.answer: str | None = None

    def accept(self, *_):
        self.answer = "ok"

    def dismiss(self):
        self.answer = "cancel"


def on_popup(target, handler, once: bool = False):
    """Like `target.on("dialog", handler)`, for in-page popups; `target` is a Page or a BrowserContext."""
    page_only = target if hasattr(target, "context") and not hasattr(target, "new_page") else None
    context = page_only.context if page_only is not None else target
    handlers = getattr(context, "_sd_popup_handlers", None)
    if handlers is None:
        handlers = []
        context._sd_popup_handlers = handlers

        def answer(source, kind, message):
            popup = Popup(kind, message)
            for entry in list(handlers):
                if entry["page"] is not None and entry["page"] != source["page"]:
                    continue
                if entry["once"]:
                    handlers.remove(entry)
                try:
                    entry["handler"](popup)
                except Exception:   # an assertion in a handler must show, not hang the popup
                    traceback.print_exc()
                    return "cancel"
                if popup.answer:
                    break
            return popup.answer or "cancel"

        context.expose_binding("__sdPopup", answer)
        context.add_init_script(f"({WATCH})()")
        for page in context.pages:
            try:
                page.evaluate(WATCH)
            except Exception:
                pass   # not navigated yet: the init script covers it
    handlers.append({"handler": handler, "once": once, "page": page_only})
