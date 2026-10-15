"""Explicit input preconditions, not a patch of Playwright's APIs.

Callers retain their business assertions. Popovers and embedded clear buttons
are intentionally not classified as overlapping peer fields.
"""
from playwright.sync_api import expect


def hit(locator):
    expect(locator).to_be_visible()
    locator.click(trial=True)
    assert locator.evaluate('''el => {
        const r = el.getBoundingClientRect();
        const x = r.left + r.width / 2, y = r.top + r.height / 2;
        const top = el.ownerDocument.elementFromPoint(x, y);
        return r.width > 0 && r.height > 0 && top && (top === el || el.contains(top));
    }'''), f'input center obscured: {locator}'


def hit_fill(locator, value, **kwargs):
    hit(locator)
    locator.click()
    locator.fill(value, **kwargs)


def hit_select(locator, *values, **kwargs):
    # Trial checks the native select without leaving its OS popup open.
    hit(locator)
    return locator.select_option(*values, **kwargs)


def peers(page, selectors):
    """Only explicitly named sibling controls; never arbitrary DOM rectangles."""
    boxes = [(s, page.locator(s).bounding_box()) for s in selectors]
    for i, (name, a) in enumerate(boxes):
        assert a, name
        for other, b in boxes[i + 1:]:
            assert b, other
            overlap = (min(a['x']+a['width'], b['x']+b['width'])-max(a['x'],b['x']),
                       min(a['y']+a['height'], b['y']+b['height'])-max(a['y'],b['y']))
            assert min(overlap) <= 1, (name, other, overlap)


def wheel_to(page, locator, area, direction=1):
    """Reach a fully visible control by real wheel, before any locator action."""
    for _ in range(35):
        if locator.evaluate('''el => {
            const r=el.getBoundingClientRect();
            if(r.top<0 || r.bottom>innerHeight || r.left<0 || r.right>innerWidth)return false;
            return [[.15,.15],[.85,.15],[.15,.85],[.85,.85],[.5,.5]].every(([x,y])=>{
                const top=document.elementFromPoint(r.left+x*r.width,r.top+y*r.height);
                return top && (top===el || el.contains(top));
            });
        }'''):
            return
        r = area.bounding_box()
        page.mouse.move(r['x']+r['width']*.5, max(5, min(page.viewport_size['height']-5, r['y']+r['height']*.5)))
        page.mouse.wheel(0, direction*400)
        page.wait_for_timeout(60)
    raise AssertionError(f'not reachable by wheel: {locator}')


def error_tail(page, locator, area):
    """Reach the last text glyph of a long error by wheel, even if taller than viewport."""
    for _ in range(40):
        if locator.evaluate('''el=>{
            const range=document.createRange();range.selectNodeContents(el);
            range.setStart(el.firstChild,Math.max(0,el.textContent.length-1));
            const r=range.getBoundingClientRect();
            const top=document.elementFromPoint(r.left+r.width/2,r.top+r.height/2);
            return r.top>=0 && r.bottom<=innerHeight && top && (top===el || el.contains(top));
        }'''):
            return
        r=area.bounding_box()
        page.mouse.move(r['x']+r['width']*.5,max(5,min(page.viewport_size['height']-5,r['y']+r['height']*.5)))
        page.mouse.wheel(0,400);page.wait_for_timeout(60)
    raise AssertionError(f'error tail unreachable by wheel: {locator}')


def single_line_button(locator):
    assert locator.evaluate('''el=>{
        const r=document.createRange();r.selectNodeContents(el);
        return [...r.getClientRects()].length===1 && el.scrollWidth<=el.clientWidth;
    }'''), f'button label wrapped or clipped: {locator}'
