export interface QuestionRuntimeService {
  questionFormDrafts: any
  screenMenuTextDrafts: any
  pendingHistoryQuestion(...args: any[]): any
  pruneQuestionFormDrafts(...args: any[]): any
}
export function createQuestionRuntime(): QuestionRuntimeService
