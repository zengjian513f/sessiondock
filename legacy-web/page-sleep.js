'use strict';

globalThis.SessionDockSleep = (() => {
  const choices = new Set([0, 5, 15, 30, 60, 120, 240]);
  const normalized = value => value !== null && value !== '' && choices.has(Number(value)) ? Number(value) : 60;
  let minutes = normalized(store.get('sleepMinutes', 60));
  let lastActivity = Date.now(), timer = 0, sleeping = false, dialog;

  function schedule() {
    clearTimeout(timer);
    if (!sleeping && minutes) timer = setTimeout(check, Math.max(1, lastActivity + minutes * 60000 - Date.now()));
  }

  function sleep() {
    sleeping = true;
    clearTimeout(timer);
    SessionDockNetwork.pause('idle');
    if (!dialog) {
      dialog = document.createElement('dialog');
      dialog.id = 'page-sleep-dialog';
      dialog.className = 'page-sleep-dialog';
      dialog.setAttribute('aria-labelledby', 'page-sleep-title');
      dialog.innerHTML = `<div class="app-float session-freeze-line">
        <span id="page-sleep-title">页面已休眠</span>
        <button class="btn" type="button" aria-label="Resume">${uiIcon('play')}<span>Resume</span></button>
      </div>`;
      dialog.addEventListener('cancel', event => event.preventDefault());
      dialog.addEventListener('keydown', event => event.stopPropagation());
      dialog.querySelector('button').onclick = () => {
        sleeping = false;
        lastActivity = Date.now();
        dialog.close();
        schedule();
        SessionDockNetwork.resume();
      };
      document.body.append(dialog);
    }
    dialog.showModal();
  }

  function check() {
    if (sleeping) return;
    // Wall time also covers a suspended computer or a throttled background tab.
    if (minutes && Date.now() - lastActivity >= minutes * 60000) sleep();
    else schedule();
  }

  function activity(event) {
    if (!event.isTrusted) return;
    check();
    if (sleeping) {
      // A first click after a suspended timer must not reach the old control.
      if (!dialog.contains(event.target)) {
        if (event.cancelable) event.preventDefault();
        event.stopImmediatePropagation();
      }
      return;
    }
    lastActivity = Date.now();
  }
  for (const type of ['pointerdown', 'pointermove', 'keydown', 'wheel', 'touchstart', 'input']) {
    addEventListener(type, activity, {capture: true, passive: false});
  }
  // Check before focus/visibility handlers can reconnect streams. Merely
  // returning to a tab isn't interaction and never dismisses the sleep dialog.
  for (const type of ['focus', 'pageshow', 'visibilitychange']) addEventListener(type, check, true);

  function configure(value) {
    minutes = normalized(value);
    document.querySelector('#setting-sleep').value = String(minutes);
    check();
  }
  document.querySelector('#setting-sleep').onchange = event => {
    store.set('sleepMinutes', normalized(event.target.value));
    lastActivity = Date.now();
    configure(event.target.value);
  };
  addEventListener('storage', event => {
    if (event.key === STORAGE_PREFIX + 'sleepMinutes' || event.key === null) {
      configure(store.get('sleepMinutes', 60));
    }
  });
  schedule();
  return Object.freeze({get minutes() {return minutes;}, get sleeping() {return sleeping;}});
})();
