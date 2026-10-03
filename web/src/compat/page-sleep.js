'use strict';
// Transitional classic-defer entry; final bootstrap calls mountSleep explicitly.
globalThis.SessionDockSleep = SessionDockOverlays.mountSleep({store, storagePrefix:STORAGE_PREFIX, network:SessionDockNetwork, settings:SessionDockSettings});
mountSettings();
