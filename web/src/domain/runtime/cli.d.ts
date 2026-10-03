export interface CliOption { keys?: string[]; key?: string; [field: string]: any }
export interface CliPrompt {
  kind?: string
  questions?: { multiple?: boolean; options?: CliOption[] }[]
}
export class SessionDockCli {
  constructor(source: string, name: string, icon: string, color: string)
  source: string
  name: string
  icon: string
  color: string
  readonly nativeHistory: boolean
  questionAnswerKeys(prompt: CliPrompt, optionIndex: number): string[] | null
  canAnswerQuestionForm(prompt: CliPrompt): boolean
  questionFormAnswerKeyGroups(prompt: CliPrompt, optionIndexes: number[]): string[][] | null
  questionCancelKeys(): string[]
  repeatedEscape(now: number, previousAt: number, context?: {busy?: boolean; empty?: boolean}): {rewind: boolean; nextAt: number}
}
export class ClaudeCli extends SessionDockCli { constructor() }
export class CodexCli extends SessionDockCli { constructor() }
export class GrokCli extends SessionDockCli { constructor() }
export class OpencodeCli extends SessionDockCli { constructor() }
export const SESSIONDOCK_CLIS: Readonly<Record<string, SessionDockCli>>
export function sessiondockCli(sourceOrUid: string | null): SessionDockCli | null
