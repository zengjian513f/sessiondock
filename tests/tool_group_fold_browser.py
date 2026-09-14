#!/usr/bin/env python3
"""Tool groups: a group the user opened stays open when later
tools or results append; the tail group opens while streaming and seals when the turn ends.

Isolated Rust server over the shared synthetic corpus; the checks run against the page's own
``appendMessages``/``sealToolTail`` on detached containers, so no native file is touched.
"""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import tempfile

from playwright.sync_api import sync_playwright

from history_parity import BINARY, build_corpus, isolated_server

EVAL = """() => {
  const tool = n => ({role:'tool', name:'exec', summary:`$ echo ${n}`,
                      text:`{"n":${n}}`, call_id:`call-${n}`});
  const box = document.createElement('div');
  appendMessages(box, [tool(1)], null, {openTail:true});
  const firstIsSingle = box.children.length === 1
    && box.firstElementChild.matches('.tool-msg');
  appendMessages(box, [tool(2)], null, {openTail:true});
  let group = box.querySelector(':scope > .grp');
  const twoBecomeOpenGroup = box.children.length === 1 && !!group
    && !group.classList.contains('folded') && group._toolItems.length === 2;
  appendMessages(box, [tool(3)], null, {openTail:true});
  group = box.querySelector(':scope > .grp');
  const nextBatchJoinsTail = box.children.length === 1
    && group._toolItems.length === 3 && !group.classList.contains('folded');
  appendMessages(box, [{role:'assistant', text:'工具段结束'}], null, {openTail:false});
  group = box.querySelector(':scope > .grp');
  const nonToolSealsGroup = group.classList.contains('folded')
    && group.nextElementSibling?.dataset.role === 'assistant';

  const idle = document.createElement('div');
  appendMessages(idle, [tool(4), tool(5)], null, {openTail:true});
  const openBeforeIdle = !idle.querySelector('.grp').classList.contains('folded');
  sealToolTail(idle);
  const idleSealsGroup = idle.querySelector('.grp').classList.contains('folded');

  const manual = document.createElement('div');
  appendMessages(manual, [tool(6), tool(7)], null, {openTail:false});
  const foldedAtRest = manual.querySelector('.grp').classList.contains('folded');
  const sameNode = manual.querySelector('.grp');
  sameNode.querySelector('.fold-toggle').click();
  const openedByUser = !sameNode.classList.contains('folded') && sameNode._userOpened === true;
  appendMessages(manual, [tool(8)], null, {openTail:false});
  const afterAppend = manual.querySelector('.grp');
  const staysOpenOnAppend = afterAppend === sameNode
    && !afterAppend.classList.contains('folded')
    && afterAppend._userOpened === true
    && afterAppend._toolItems.length === 3;
  const innerKept = afterAppend.querySelectorAll(':scope > .tool-entry').length === 3;
  sealToolTail(manual);
  const sealRespectsUser = !manual.querySelector('.grp').classList.contains('folded')
    && manual.querySelector('.grp')._userOpened === true;
  manual.querySelector('.grp .disclosure').click();
  const userCanRefold = manual.querySelector('.grp').classList.contains('folded')
    && !manual.querySelector('.grp')._userOpened;

  const pairing = document.createElement('div');
  appendMessages(pairing, [tool(9), tool(10)], null, {openTail:false});
  const pairGroup = pairing.querySelector('.grp');
  pairGroup.querySelector('.fold-toggle').click();
  appendMessages(pairing, [{role:'tool_result', call_id:'call-10', text:'done 10'}],
                 null, {openTail:false});
  const afterResult = pairing.querySelector('.grp');
  const resultKeepsUserOpen = afterResult === pairGroup
    && !afterResult.classList.contains('folded')
    && afterResult._userOpened === true
    && afterResult._toolItems.length === 2
    && !!afterResult._toolItems[1].result
    && afterResult.querySelectorAll(':scope > .tool-entry').length === 2
    && afterResult.querySelectorAll(':scope > .tool-entry .tool-status').length === 1;

  // Rust media continuation rides along with a paired result (appendToolResult keeps media_more).
  const media = document.createElement('div');
  appendMessages(media, [tool(11), tool(12)], null, {openTail:false});
  appendMessages(media, [{role:'tool_result', call_id:'call-12', text:'done 12',
    media:[{src:'/api/media/' + 'a'.repeat(32), alt:'图', width:4, height:4}],
    media_more:{remaining:2, total:3, cursor:'b'.repeat(32)}}], null, {openTail:false});
  const resultMediaKept = media.querySelectorAll('.tool-entry .media-gallery img').length === 1
    && media.querySelectorAll('.tool-entry .media-more[data-media-cursor]').length
      === (SessionDockCapabilities.config.media_continuation === true ? 1 : 0);
  return {firstIsSingle, twoBecomeOpenGroup, nextBatchJoinsTail,
          nonToolSealsGroup, openBeforeIdle, idleSealsGroup,
          foldedAtRest, openedByUser, staysOpenOnAppend, innerKept,
          sealRespectsUser, userCanRefold, resultKeepsUserOpen, resultMediaKept};
}"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-tool-group-fold-") as directory:
        data = build_corpus(Path(directory))
        with isolated_server(data, args.binary) as (base, _opener), sync_playwright() as playwright:
            launch = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**launch)
            try:
                context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                page = context.new_page()
                errors = []
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.goto(base, wait_until="networkidle")
                page.wait_for_function("S.sessions.length > 0")
                page.evaluate("uid => openSession(uid)", data.uid("claude-branch"))
                page.wait_for_selector("#msgs .msg")
                result = page.evaluate(EVAL)
                failed = [name for name, ok in result.items() if not ok]
                assert not failed, (failed, result)
                assert not errors, errors
                context.close()
            finally:
                browser.close()
    print("PASS tool group fold browser:", ", ".join(result), flush=True)


if __name__ == "__main__":
    main()
