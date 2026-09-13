#!/usr/bin/env python3
"""Real console button DOM with synthetic per-node resume capabilities only.

No service, CLI, terminal attachment or user history is opened. The production
node availability functions are loaded unchanged into a minimal browser page.
"""
import os
import re
from pathlib import Path
from playwright.sync_api import sync_playwright, expect

ROOT = Path(__file__).resolve().parents[1]
SOURCE = (ROOT / 'legacy-web/nodes.js').read_text()
FUNCTIONS = '\n'.join([
    SOURCE[SOURCE.index('function nodeOf('):SOURCE.index('function nodeSelected(')],
    SOURCE[SOURCE.index('function consoleUnavailableReason('):SOURCE.index('function showConsoleToast(')],
    SOURCE[SOURCE.index('function paintConsoleAvailability('):SOURCE.index('function consoleButtonMarkup(')],
])


def main():
    with sync_playwright() as p:
        launch = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = p.chromium.launch(**launch)
        try:
            page = browser.new_page()
            page.set_content('<button id="a-term">接管会话</button>')
            page.add_script_tag(content='''
                const HUB_MODE = true, nid = 'a'.repeat(32), uid = `claude:${nid}~synthetic`;
                const nid2 = 'b'.repeat(32), uid2 = `claude:${nid2}~synthetic`;
                const AgentHubCapabilities = {config:{backend:'rust'}, allows:()=>true};
                const T = {enabled:true,listLoaded:true,listError:'',resume_sources:{},ended:new Map(),pending:[]};
                const Nodes = {list:[{id:nid,name:'Lyra fixture'},{id:nid2,name:'Other fixture'}],errors:new Map(),capabilities:{}};
                const ConsoleUI = {errors:new Map(),busy:new Set()}, SOURCES = {claude:{name:'Claude'}};
                function takeover(){} function Terminal(){} function FitAddon(){}
                function sessionTermMeta(){return {source:'claude'}}
                function linkedTermSession(){return null} function showConsoleToast(){}
            ''' + FUNCTIONS)
            button = page.locator('#a-term')
            def apply(capability, *, ended=False, top_level=False):
                page.evaluate('''({capability,ended,top_level})=>{
                    Nodes.capabilities[nid]=capability;
                    T.resume_sources={claude:top_level};
                    T.ended=ended?new Map([[uid,{reason:'已退出且不可恢复'}]]):new Map();
                    paintConsoleAvailability(document.querySelector('#a-term'),uid);
                }''', {'capability': capability, 'ended': ended, 'top_level': top_level})
            ready = {'enabled': True, 'sources': {'claude': True}, 'resume_sources': {'claude': True}}
            for ended in [False, True]:
                apply(ready, ended=ended)
                expect(button).to_have_attribute('data-unavailable', 'false')
            for capability in [
                {'enabled': True, 'sources': {'claude': True}, 'resume_sources': {'claude': False}},
                {'enabled': True, 'sources': {'claude': True}},
                {'enabled': False, 'unavailable_reason': 'fixture offline', 'sources': {'claude': True}, 'resume_sources': {'claude': True}},
                {'enabled': True, 'sources': {'claude': False}, 'resume_sources': {'claude': True}},
            ]:
                apply(capability, top_level=True)
                expect(button).to_have_attribute('data-unavailable', 'true')
                expect(button).to_have_attribute('aria-label', re.compile('控制台不可用'))
            apply({'enabled': True, 'sources': {'claude': True}, 'resume_sources': {'claude': False}}, ended=True, top_level=True)
            expect(button).to_have_attribute('data-unavailable', 'true')
            expect(button).to_have_attribute('aria-label', re.compile('已退出且不可恢复'))
            apply(ready)
            page.evaluate("Nodes.capabilities[nid2]={enabled:true,sources:{claude:true},resume_sources:{claude:false}}; paintConsoleAvailability(document.querySelector('#a-term'),uid2)")
            expect(button).to_have_attribute('data-unavailable', 'true')
            page.evaluate("paintConsoleAvailability(document.querySelector('#a-term'),uid)")
            expect(button).to_have_attribute('data-unavailable', 'false')
            page.evaluate("paintConsoleAvailability(document.querySelector('#a-term'),uid,'child-agent')")
            expect(button).to_have_attribute('data-unavailable', 'true')
            expect(button).to_have_attribute('aria-label', re.compile('子代理没有独立控制台'))
            print('PASS hub console button: per-node resume enables unlinked/exited sessions; global flags cannot enable unresumable, unknown, offline, missing-CLI or child views')
        finally:
            browser.close()


if __name__ == '__main__':
    main()
