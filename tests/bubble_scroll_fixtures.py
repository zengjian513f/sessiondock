"""Real pointer checks for shared read-only bubble scrolling without bubble outlines."""
# run_validation: skip
import re

from playwright.sync_api import expect


def check_scroll_inset(body):
    """The clipping edge must remain inset even after the content scrolls away."""
    gap = body.evaluate("""e => {
      const r = e.getBoundingClientRect();
      const frame = e.parentElement.matches('.tool-text-frame, .event-detail-frame')
        ? e.parentElement : e;
      const f = frame.getBoundingClientRect();
      const scale = r.height / e.offsetHeight;
      return {top: (r.top - f.top) / scale + e.clientTop,
        bottom: (f.bottom - r.bottom) / scale + e.offsetHeight - e.clientHeight - e.clientTop};
    }""")
    # client/offset sizes round independently under the supported interface zoom.
    assert gap['top'] >= 5 and gap['bottom'] >= 5, gap


def check_bubble_scroll(page, owner, body, outside):
    body.scroll_into_view_if_needed()
    expect(body).to_be_visible()
    check_scroll_inset(body)
    assert body.evaluate('(e) => e.scrollHeight > e.clientHeight + 1')
    page.wait_for_function("""element => element.scrollHeight - element.clientHeight - element.scrollTop < 2""",
                           arg=body.element_handle())
    tail = body.evaluate('(e) => e.scrollTop')
    body.click(position={'x': 35, 'y': 35})
    expect(owner).to_have_class(re.compile('text-scroll-active'))
    assert owner.evaluate('(e) => getComputedStyle(e).outlineStyle') == 'none'
    body.hover()
    page.mouse.wheel(0, -140)
    page.wait_for_function('([element, tail]) => element.scrollTop < tail', arg=[body.element_handle(), tail])
    before = body.evaluate('(e) => e.scrollTop')
    outside.click()
    expect(owner).not_to_have_class(re.compile('text-scroll-active'))
    assert owner.evaluate('(e) => getComputedStyle(e).outlineStyle') == 'none'
    body.scroll_into_view_if_needed()
    body.hover()
    page.mouse.wheel(0, -140)
    page.wait_for_timeout(120)
    assert body.evaluate('(e) => e.scrollTop') == before
