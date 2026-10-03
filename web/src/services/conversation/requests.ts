// Existing request registries are independent of rendered cards. The cache/CLI
// bridge retains native scope, authorization and checkpoint checks.
export const historyPageRequests=new Map<string,any>()
export const mediaPageRequests=new Map<string,any>()
