export function sessionStopConcurrency(preferences: {get(key: string, fallback: number): unknown}) {
  const value = Number(preferences.get('stopConcurrency', 6))
  return [1, 2, 4, 6, 8, 12, 16].includes(value) ? value : 6
}
