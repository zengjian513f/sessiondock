export interface SearchOptions {case: boolean; word: boolean; regex: boolean; mode: string}
export interface SearchRow {uid: string; updated?: string; [key: string]: unknown}
export interface SearchNode {id: string; name: string; done: number; total: number | null; state: string; online?: boolean}
export interface SearchData {results?: SearchRow[]; error?: string; truncated?: boolean; partial?: boolean; errors?: {node_id?: string; name: string}[]; nodes?: {id: string; online?: boolean}[]; [key: string]: unknown}
export interface SearchState {term: string; opts: SearchOptions; results: SearchRow[] | null; off: Set<string>; cur: number; markCapped: boolean; autoOpen: number}
export interface SearchBridge {
 query(): string
 read(): SearchState
 write(patch: Partial<Pick<SearchState, 'term' | 'opts' | 'results' | 'cur' | 'markCapped'>>): void
 clearFolds(): void
 renderSide(): void
 showSessionCount(): void
 persistOptions(options: SearchOptions): void
 allowsSearch(): boolean
 appUrl(path: string): string
 hub: boolean
 sources: string[]
 nodes(): {id: string; name: string; online?: boolean}[]
 selectedNodeIds(): string[]
 clearNodeErrors(): void
 applyNodeState(data: SearchData): void
 count(rows: SearchRow[]): number
 escape(text: string): string
 showMatches(rows: SearchRow[]): void
}
export type RegexRanges = [number, number][] & {matched?: boolean}
