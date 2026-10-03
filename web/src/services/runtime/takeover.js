

export function createTakeover({terminal,core,dom,composer,presentation}){
function renderTakeoverBtn() {
  const b = document.querySelector('#a-term');
  if (!b) return;
  const name = terminal().takenOver(core.state.selection.sel);
  const replacement = name ? null
    : terminal().linkedTermSession(core.state.selection.sel, { followReplacement: true });
  const paneOpen = !!name && !document.querySelector('#termpane').classList.contains('hidden');
  const ptyOnly = typeof terminal().sessionTerminalFirst === 'function' && terminal().sessionTerminalFirst(core.state.selection.sel);
  const switchToChat = paneOpen && (ptyOnly ? terminal().state.mode === 'full' : (core.environment.MOBILE.matches || terminal().state.mode === 'full'));
  const terminalVisible = paneOpen && (ptyOnly || core.environment.MOBILE.matches || terminal().state.mode !== 'collapsed');
  const label = replacement ? '切换到当前会话终端'
    : !name ? '接管会话'
    : ptyOnly ? (switchToChat ? '切换到对话' : '切换到终端')
    : core.environment.MOBILE.matches ? (paneOpen ? '切换到对话' : '切换到终端')
    : switchToChat ? '切换到对话' : '切换到终端';
  Object.assign(presentation.consoleButton,{icon:switchToChat?'chat':'terminal',title:label,label,expanded:String(terminalVisible),on:!!name});
  core.nodes.paintConsoleAvailability(b, core.state.selection.sel, core.state.selection.agent);
  composer().renderComposer();
  if (typeof core.diagnostics.auditConsoleButton === 'function') core.diagnostics.auditConsoleButton('takeover-btn');
}
return {renderTakeoverBtn}
}
