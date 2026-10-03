export interface InstallState {
  text: string
  disabled: boolean
  title: string
}

export interface SettingsValues {
  scale: number
  font: string
  theme: string
  sleep: number
  cache: number
  stopConcurrency: number
  pasteFiles: boolean
}

export interface SettingsState extends SettingsValues {
  pwa: InstallState
}

// Operations stay with the existing preference and service owners.
export interface SettingsBridge {
  read(): SettingsValues
  scale(value: string | number, persist: boolean): void
  scaleIndicator(value: number, active?: boolean): void
  font(value: string): void
  theme(value: string): void
  sleep(value: string): void
  cache(value: string): void
  stopConcurrency(value: string): void
  pasteFiles(value: boolean): void
  pwa: {
    install(): Promise<void>
    takeOver(render: (state: InstallState) => void): InstallState
  }
}
