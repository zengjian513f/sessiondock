export interface FormulaRuntimeService {
  formulaLoading: any
  formulaRoots: any
  renderFormulae(...args: any[]): any
}
export function createFormulaRuntime(): FormulaRuntimeService
