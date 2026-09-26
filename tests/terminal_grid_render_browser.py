"""Grid renderer pixels in real Chromium (no server): after a full paint a row
of tall glyphs (block elements, ❯, accented capitals, CJK, emoji) leaves no
pixel in the neighbouring rows' boxes, the cell height lands on whole device
pixels for fractional dpr, and a multi-row background shows no seam. Modules
come from legacy-web/ by default; `--base URL` fetches them from a running
instance instead (production check of the deployed assets).

Before the per-row clip this failed with 20–90 lit pixels above/below the
tall row and a 21.25/25.5-device-pixel row height at dpr 1.25/1.5.
"""
import argparse
import sys
from pathlib import Path

from playwright.sync_api import sync_playwright

REPO = Path(__file__).resolve().parent.parent
WEB = REPO / "legacy-web"
BASE = ""
def row(text, bg=-1):
    return {"s": [[text, -1, bg, 0]], "w": 24}
SNAP = {"t": "snapshot", "seq": 1, "reset": True, "cols": 24, "rows": 5, "history": [], "history_total": 0,
        "cursor": {"x": 0, "y": 4, "visible": False}, "modes": {},
        "grid": [row(""), row("▓▓❯ ÀÇ 中文 \U0001f552 |"), row(""), row("▓▓▓▓▓▓", 0x1000000 + 0x404040), row("▓▓▓▓▓▓", 0x1000000 + 0x404040)]}
HTML = "<!doctype html><html><body style='margin:0;background:#000'><input id=scale type=range min=30 max=150 value=100><div id=host style='width:400px;height:200px'></div></body></html>"
def run(browser, dpr):
    ctx = browser.new_context(viewport={"width": 500, "height": 300}, device_scale_factor=dpr)
    page = ctx.new_page(); errors = []; page.on("pageerror", lambda e: errors.append(str(e)))
    def serve(route):
        p = route.request.url.split("http://grid.test/", 1)[1].split("?")[0]
        if p == "" or p == "index.html":
            return route.fulfill(status=200, content_type="text/html", body=HTML)
        if BASE:
            upstream = ctx.request.get(BASE.rstrip("/") + "/" + p)
            return route.fulfill(status=upstream.status, content_type="text/javascript", body=upstream.body())
        f = WEB / p
        if f.is_file():
            return route.fulfill(status=200, content_type="text/javascript", body=f.read_text())
        route.fulfill(status=404, body="")
    page.route("http://grid.test/**", serve)
    page.goto("http://grid.test/")
    result = page.evaluate("""async snap => {
      const {GridTerm} = await import('/grid/facade.js');
      const term = new GridTerm({fontSize: 14, lineHeight: 1.2});
      term.open(document.querySelector('#host'));
      term.write(JSON.stringify(snap) + '\\n');
      await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)));
      document.querySelector('#scale').oninput = event => {
        document.documentElement.style.zoom = event.target.value / 100;
        term.refresh(0, term.rows - 1);
      };
      window.inspectPixels = () => {
      const canvas = document.querySelector('#host canvas');
      const r = term.renderer, dpr = r.dpr, ch = r.cellHeight, cw = r.cellWidth;
      const ctx = canvas.getContext('2d');
      const bg = getComputedStyle(canvas).backgroundColor;
      const box = (rowIndex, colFrom, colTo) => {
        const x = Math.round(colFrom * cw * dpr), w = Math.round((colTo - colFrom) * cw * dpr);
        const y = Math.round(rowIndex * ch * dpr), h = Math.round(ch * dpr);
        const d = ctx.getImageData(x, y, w, h).data; let non = 0;
        for (let i = 0; i < d.length; i += 4) if (d[i] > 8 || d[i+1] > 8 || d[i+2] > 8) non++;
        return non;
      };
      // rows 0 and 2 are blank: any lit pixel there is spill from row 1
      const spillAbove = box(0, 0, 24), spillBelow = box(2, 0, 24);
      // row 1 itself must have painted something
      const glyphs = box(1, 0, 24);
      // multi-row background (rows 3-4): count pixels darker than the block colour along the seam line
      const seamY = Math.round(4 * ch * dpr), x0 = 0, w = Math.round(6 * cw * dpr);
      let seamDark = 0;
      for (const yy of [seamY - 1, seamY]) { const d = ctx.getImageData(x0, yy, w, 1).data; for (let i = 0; i < d.length; i += 4) if (d[i] < 0x30) seamDark++; }
      return {dpr, ch, cw, chDevice: ch * dpr, spillAbove, spillBelow, glyphs, seamDark, seamWidth: w * 2, canvas: [canvas.width, canvas.height], displayedWidth: canvas.getBoundingClientRect().width, deviceDpr: devicePixelRatio};
      };
      return window.inspectPixels();
    }""", SNAP)
    for scale in (150, 137, 30, 100):
        slider = page.locator('#scale')
        slider.focus()
        slider.press('Home')
        for _ in range(scale - 30):
            slider.press('ArrowRight')
        result = page.evaluate('inspectPixels()')
        assert abs(result['dpr'] - dpr * scale / 100) < 1e-5, (scale, result)
        assert abs(result['canvas'][0] - result['displayedWidth'] * dpr) <= 1.1, (scale, result)
        assert result['spillAbove'] == result['spillBelow'] == result['seamDark'] == 0, (scale, result)
        assert result['glyphs'] > 0 and abs(result['chDevice'] - round(result['chDevice'])) < 1e-6, (scale, result)
    print(f"dpr={dpr}: cellHeight*dpr={result['chDevice']:.3f} spillAbove={result['spillAbove']} spillBelow={result['spillBelow']} glyphs={result['glyphs']} seamDark={result['seamDark']}/{result['seamWidth']} errors={errors}")
    ctx.close()
    return result
def main():
    global BASE
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", default="", help="fetch grid modules from this running instance instead of legacy-web/")
    BASE = parser.parse_args().base
    with sync_playwright() as p:
        b = p.chromium.launch(headless=True)
        ok = True
        for dpr in (1, 1.25, 1.5):
            r = run(b, dpr)
            ok &= (r["spillAbove"] == 0 and r["spillBelow"] == 0 and r["glyphs"] > 0 and r["seamDark"] == 0
                   and abs(r["chDevice"] - round(r["chDevice"])) < 1e-6)
        b.close()
    if not ok:
        raise SystemExit("FAIL terminal_grid_render_browser")
    print("PASS terminal_grid_render_browser: no glyph spill into neighbouring rows, device-aligned rows, no background seam (dpr 1/1.25/1.5)")


if __name__ == "__main__":
    main()
