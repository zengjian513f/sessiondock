'use strict';
// Transitional classic-defer entry; Vue owns the native dialog.
globalThis.SessionDockResources = SessionDockOverlays.mountResources({appUrl, selection:() => S.sessions?.find(row => row.uid === S.sel) || null});
