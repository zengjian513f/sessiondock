(() => {
  Object.assign(globalThis,{appAlert:SessionDockSessionUi.appAlert,appConfirm:SessionDockSessionUi.appConfirm,floatStack:SessionDockSessionUi.floatStack});
  // Use the same native hover hint as available controls.
  const normalTitles = new WeakMap();
  globalThis.setControlUnavailable = (control, reason) => {
    if (!control) return;
    const previous = control.dataset.unavailableReason;
    if (reason) {
      if (!previous || control.title !== previous) normalTitles.set(control, control.title);
      control.dataset.unavailableReason = reason;
      control.setAttribute('aria-disabled', 'true');
      control.setAttribute('aria-description', reason);
      control.title = reason;
    } else {
      if (previous && control.title === previous) control.title = normalTitles.get(control) || '';
      normalTitles.delete(control);
      delete control.dataset.unavailableReason;
      control.removeAttribute('aria-disabled');
      control.removeAttribute('aria-description');
    }
  };
  for (const type of ['click', 'dblclick', 'contextmenu']) document.addEventListener(type, event => {
    if (!event.target.closest?.('[data-unavailable-reason]')) return;
    event.preventDefault(); event.stopImmediatePropagation();
  }, true);
})();
