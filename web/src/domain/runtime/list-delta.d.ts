export interface ListRow { [field: string]: any }
export interface ListPatch {
  key: string
  remove: string[]
  upsert: {index: number; id?: string; row?: ListRow; set?: ListRow; unset?: string[]; agents?: ListPatch}[]
  order?: string[]
}
export function expandRows(baseline: ListRow[], patch: ListPatch): ListRow[]
export function expand(wire: ListRow, baseline: ListRow | undefined): ListRow
