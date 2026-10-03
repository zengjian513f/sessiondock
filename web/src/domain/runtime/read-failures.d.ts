export function transientReadFailure(error: any): boolean
export function retryableReadFailure(failure: any): boolean
export const RETRY_BASE_MS: number
export const RETRY_MAX_MS: number
export function retryDelay(attempt: number): number
