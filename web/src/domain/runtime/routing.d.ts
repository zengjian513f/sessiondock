export function createSessionRouting(catalog: {sessions: any[]}, deepNode: () => string): {
  uidOfDeepLink(spec: string): string | null
  agentOfDeepLink(spec: string): {uid: string; agent: string} | null
  routeSession(spec: string): {uid: string; agent: string | null} | null
}
