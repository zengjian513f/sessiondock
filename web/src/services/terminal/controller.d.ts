import type { TerminalBridge, TerminalController } from './bridge'
import type { TerminalUiState } from '../../stores/terminal'
export function createTerminalController(bridge: TerminalBridge, state: TerminalUiState, publish: () => void): TerminalController
