export interface TimelineService {
  timelinePath: any
  TIMELINE_COLORS: any
  timelineColors: any
  timelineDirectoryColors(...args: any[]): any
  timelinePlanCache: any
  timelinePathPlans(...args: any[]): any
  timelineFitContext: any
  fitTimelineDirectories(...args: any[]): any
  timelineFitFrame: any
  scheduleTimelineFit(...args: any[]): any
  dayKey: any
  start(...args: any[]): any
}
export function createTimeline(dependencies: {core: import('./core').RuntimeCore; dom: () => any; sidebarView: () => any}): TimelineService
