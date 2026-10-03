export interface AppearanceDependencies {
  preferences: {get<T>(key: string, fallback: T): T; set(key: string, value: unknown): void}
  choices: Record<string, string>
  updateSettings(value: {theme?: string; font?: string}): void
  refreshTerminalPreferences(redraw: boolean): void
}
export function createAppearance(deps: AppearanceDependencies) {
  let mounted = false
  const media = matchMedia('(prefers-color-scheme: dark)')
  function applyTheme(choice = deps.preferences.get('theme', 'system'), persist = false) {
    if (!['system','light','dark'].includes(choice)) choice = 'system'
    if (persist) deps.preferences.set('theme',choice)
    document.documentElement.dataset.theme = choice === 'system' ? (media.matches ? 'dark' : 'light') : choice
    if (mounted) deps.updateSettings({theme:choice})
    deps.refreshTerminalPreferences(true)
  }
  function applyFont(choice = deps.preferences.get('font', 'ubuntu'), persist = false) {
    if (!deps.choices[choice]) choice = 'ubuntu'
    if (persist) deps.preferences.set('font',choice)
    document.documentElement.style.setProperty('--terminal-font',deps.choices[choice]!)
    if (mounted) deps.updateSettings({font:choice})
    deps.refreshTerminalPreferences(false)
  }
  function start() {
    const changed = () => {if (deps.preferences.get('theme','system') === 'system') applyTheme('system')}
    media.addEventListener('change',changed)
    applyTheme()
    applyFont()
    return () => media.removeEventListener('change',changed)
  }
  return {applyTheme,applyFont,start, settingsMounted() {mounted = true}}
}
