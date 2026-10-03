const $ = selector => document.querySelector(selector);
import * as Shell from '../../migration/shell'

import * as Settings from '../../migration/settings'
import * as Machines from '../../migration/machines'
import * as Overlays from '../../migration/overlays'

import {SOURCES} from '../../domain/runtime/sources'
import {SESSIONDOCK_CLIS} from '../../domain/runtime/cli.js'

export function createSettingsRuntime({terminal,core,sleep,bulk,appearance,capabilities,sidebarResources}) {
let settingsMounted = false;
function machineTargets() {
  if (typeof terminal().state === 'undefined' || !terminal().state.listLoaded) return [];
  if (!core.environment.HUB_MODE) {
    return terminal().state.enabled
      ? [{id: '', name: '本机', color: '', online: true, local: true, enabled: true,
          renderer: localConsoleRenderer()}] : [];
  }
  // 按注册表顺序列全部机器，停用的原位留着（只剩勾选框能把它接回来），不往后挪
  const machines = core.state.nodes.machines.length ? core.state.nodes.machines : core.state.nodes.list;
  return machines.map(node => {
    const on = node.enabled !== false;
    const cap = on ? core.state.nodes.capabilities[node.id] || {} : {};
    return {id: node.id, name: node.name, color: node.color || '',
            online: on ? node.online : null, local: false, enabled: on,
            terminal: !!cap.enabled, renderer: node.renderer === 'xterm' ? 'xterm' : 'grid'};
  });
}

const CONSOLE_RENDERERS = [['grid', '服务端网格（默认）'], ['xterm', 'xterm.js（浏览器解析）']];

function localConsoleRenderer() {
  return core.preferences.get('consoleRenderer', 'grid') === 'xterm' ? 'xterm' : 'grid';
}

function consolePasteFilesEnabled() {
  return core.preferences.get('consolePasteFiles', false) === true;
}

function settingsValues() {
  return {
    scale: Shell.interfaceScale(), font: core.preferences.get('font', 'ubuntu'),
    theme: core.preferences.get('theme', 'system'), cache: core.cache.settings.limitMb,
    sleep: sleep().minutes, stopConcurrency: bulk().sessionStopConcurrency(),
    pasteFiles: consolePasteFilesEnabled(),
  };
}

function mountSettings() {
  Shell.takeOverResourceToggle(() => sidebarResources().toggle());
  Machines.mount({
    readTargets: machineTargets,
    fetch:(...args)=>core.network.fetch(...args), appUrl:core.environment.appUrl,
    sourceNames: SOURCES,
    sources: Object.keys(SESSIONDOCK_CLIS),
    rendererOptions: CONSOLE_RENDERERS,
    offlineReason: target => typeof core.nodes.nodeOfflineReason === 'function'
      ? core.nodes.nodeOfflineReason(core.state.nodes.list.find(n => n.id === target.id) || {}) : '离线',
    setLocalRenderer: value => core.preferences.set('consoleRenderer', value),
    applyDisplay: (target, changed) => {
      const node = core.state.nodes.list.find(n => n.id === target.id);
      if (node) Object.assign(node, {name: changed.name, color: changed.color});
    },
    applyRenderer: (target, renderer) => {
      const node = [...core.state.nodes.machines, ...core.state.nodes.list].find(n => n.id === target.id);
      if (node) node.renderer = renderer;
    },
    applyOrder: machines => { core.state.nodes.machines = machines; },
    loadNodes:core.nodes.loadNodes,
    loadSessions: () => core.list.loadSessions(true),
    refreshLive: () => core.live.refreshLive(true),
    loadTermList: () => typeof terminal().loadTermList === 'function' ? terminal().loadTermList() : null,
    renderNodes: () => { if (typeof core.nodes.renderNodes === 'function') core.nodes.renderNodes(); },
  });

  Settings.mount({
    read: settingsValues,
    scale: Shell.applyInterfaceScale,
    scaleIndicator: Shell.showScaleIndicator,
    font: value => appearance().applyFont(value, true),
    theme: value => appearance().applyTheme(value, true),
    sleep: value => sleep().configure(value, true),
    cache: value => {
      core.cache.settings.limitMb = Math.max(0, +value || 0);
      core.cache.settings.maxBytes = core.cache.settings.limitMb ? core.cache.settings.limitMb * 1024 * 1024 : Infinity;
      core.preferences.set('cacheMb', core.cache.settings.limitMb);
      core.cache.trimCache();
      Settings.update({cache: core.cache.settings.limitMb});
    },
    stopConcurrency: value => {
      core.preferences.set('stopConcurrency', Number(value));
      Settings.update({stopConcurrency: bulk().sessionStopConcurrency()});
    },
    pasteFiles: value => {
      core.preferences.set('consolePasteFiles', value === true);
      Settings.update({pasteFiles: consolePasteFilesEnabled()});
    },
    pwa: Overlays.pwa,
  }, {read: () => core.preferences.get('settingsTab', 'appearance'), save: value => core.preferences.set('settingsTab', value)});
  settingsMounted = true;appearance().settingsMounted();
}
return {machineTargets,CONSOLE_RENDERERS,localConsoleRenderer,consolePasteFilesEnabled,settingsValues,mountSettings};
}
