"""Synthetic long content and real wheel helpers for embedded session views."""


def route_content(page):
    def info(route):
        data = route.fetch().json()
        item = dict(uid='codex:synthetic-related', sid='12345678-1234-1234-1234-123456789abc',
                    title='关联会话 Chinese / English 长标题 ' * 4, source='codex', available=False,
                    distance=2, kind='continues', status='confirmed', node_name='synthetic-node')
        data.update(ancestors=[dict(item, uid=f'codex:related-{i}') for i in range(35)],
                    descendants=[], predecessors=[], successors=[])
        route.fulfill(json=data)
    page.route('**/api/session/info?*', info)
    metrics = {'cpu_cores': {'value': 2.5, 'status': 'ok'}}
    page.route('**/api/session/resources?*', lambda route: route.fulfill(json={
        'totals': metrics, 'nodes': [dict(node_id=f'node-{i}', node_name=f'合成机器 compute-{i}',
        status='ok', metrics=metrics, diagnostic={'state':'unsupported'}) for i in range(6)]}))


def scroll_metrics(locator):
    return locator.evaluate("""e => ({top:e.scrollTop, height:e.clientHeight, total:e.scrollHeight,
        width:e.clientWidth, totalWidth:e.scrollWidth, overflow:getComputedStyle(e).overflowY})""")


def wheel_to_edge(page, locator, direction=1):
    # Only mouse input moves the page. DOM reads are measurements, never scrolling.
    box = page.locator('#session-content-view').bounding_box()
    page.mouse.move(box['x'] + box['width'] * .6, min(box['y'] + 160, box['y'] + box['height'] * .6))
    before = scroll_metrics(locator)
    page.mouse.wheel(0, direction * 650)
    for _ in range(20):
        page.wait_for_timeout(50)
        after = scroll_metrics(locator)
        if (after['top'] - before['top']) * direction > 0:
            break
    assert (after['top'] - before['top']) * direction > 0, ('wheel did not scroll', before, after)
    for _ in range(35):
        after = scroll_metrics(locator)
        if (direction > 0 and after['total'] - after['height'] - after['top'] <= 2) or (direction < 0 and after['top'] <= 1):
            break
        page.mouse.wheel(0, direction * 1600)
        page.wait_for_timeout(80)
    else:
        raise AssertionError(('wheel did not reach edge', after))
    assert after['totalWidth'] <= after['width'] + 1, after
    return before, after


def click_info_control(page, selector):
    # Playwright's cross-frame locator click misses CSS-zoomed iframe controls.
    # Convert the child's CSS coordinates to the parent's actual mouse pixels.
    frame = page.locator('#session-content-view iframe')
    outer = frame.bounding_box()
    size = frame.evaluate('e => ({width:e.clientWidth,height:e.clientHeight})')
    rect = page.frame_locator('#session-content-view iframe').locator(selector).evaluate(
        'e => {const r=e.getBoundingClientRect();return {x:r.x,y:r.y,width:r.width,height:r.height}}')
    page.mouse.click(outer['x'] + (rect['x'] + rect['width']/2) * outer['width']/size['width'],
                     outer['y'] + (rect['y'] + rect['height']/2) * outer['height']/size['height'])
