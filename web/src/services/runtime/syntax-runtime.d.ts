export interface SyntaxRuntimeService {
  clearSyntaxPaint(...args: any[]): any
  paintToolOutputDiff(...args: any[]): any
  syntaxWorker: any
  syntaxBusy: any
  syntaxFrame: any
  syntaxQueue: any
  syntaxGenerations: any
  scheduleSyntax(...args: any[]): any
  paintSyntax(...args: any[]): any
}
export function createSyntaxRuntime(dependencies: {dom: () => any; conversationRenderer: () => any; core: import('./core').RuntimeCore}): SyntaxRuntimeService
