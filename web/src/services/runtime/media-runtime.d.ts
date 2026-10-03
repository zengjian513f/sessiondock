export interface MediaRuntimeService {
  readonly diagnosticActive:number
  safeMediaSrc: any
  lazyMediaEnabled: any
  mediaContinuationEnabled: any
  diagnoseMedia: any
  reloadMediaSession: any
  forgetMediaDiagnostic: any
  captureView: any
  imageHtml(...args: any[]): any
  mediaMoreInfo(...args: any[]): any
  mediaPageRequests: any
  currentMediaPage(...args: any[]): any
  validateMediaPage: any
  fetchMediaPage(...args: any[]): any
  mediaPageFailure(...args: any[]): any
  loadMediaContinuation(...args: any[]): any
  start(...args: any[]): any
}
export function createMediaRuntime(dependencies: {dom: () => any; core: import('./core').RuntimeCore; capabilities: import('./core').RuntimeCapabilities}): MediaRuntimeService
