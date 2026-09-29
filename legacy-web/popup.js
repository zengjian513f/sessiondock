/* 统一弹窗：页面中央的提示框、确认框和浮动提示卡，取代浏览器原生 alert/confirm。
 *
 * appAlert(message) / appConfirm(message) 以模态 <dialog class="app-dialog app-popup">
 * 显示在页面中央，样式与新建会话、回收站等对话框一致；返回 Promise（确认框为布尔值）。
 * 消息含空行时，第一行作标题、其余作正文；完整原文留在 data-message 上。
 * Esc 或 × 等同取消。浮动提示卡（版本更新、登录环境、操作状态）放进 floatStack()：
 * 页面中央一列，不遮挡操作、不抢焦点。 */
(() => {
  function split(message, fallback) {
    const text = String(message ?? '');
    const at = text.indexOf('\n\n');
    const head = at > 0 ? text.slice(0, at).trim() : '';
    if (head && !head.includes('\n') && head.length <= 80) {
      return {title: head, body: text.slice(at + 2).trim()};
    }
    return {title: fallback, body: text};
  }

  function popup(type, message, buttons) {
    const {title, body} = split(message, type === 'confirm' ? '请确认' : '提示');
    const dialog = document.createElement('dialog');
    dialog.className = 'app-dialog app-popup';
    dialog.dataset.popupType = type;
    dialog.dataset.message = String(message ?? '');
    const id = `app-popup-${Math.random().toString(36).slice(2)}`;
    dialog.setAttribute('aria-labelledby', id);
    const panel = document.createElement('div');
    panel.className = 'app-popup-panel';
    const head = document.createElement('div');
    head.className = 'modal-head';
    const heading = document.createElement('div');
    const h2 = document.createElement('h2');
    h2.id = id;
    h2.textContent = title;
    heading.appendChild(h2);
    const close = document.createElement('button');
    close.type = 'button';
    close.className = 'modal-close';
    close.title = '关闭';
    close.setAttribute('aria-label', '关闭');
    close.textContent = '×';
    head.append(heading, close);
    panel.appendChild(head);
    if (body) {
      const text = document.createElement('div');
      text.className = 'app-popup-message';
      text.textContent = body;
      panel.appendChild(text);
    }
    const actions = document.createElement('div');
    actions.className = 'modal-actions';
    panel.appendChild(actions);
    dialog.appendChild(panel);
    return new Promise(resolve => {
      let answered = false;
      const finish = value => {
        if (answered) return;
        answered = true;
        if (dialog.open) dialog.close();
        dialog.remove();
        resolve(value);
      };
      let primary = null;
      for (const spec of buttons) {
        const button = document.createElement('button');
        button.type = 'button';
        button.className = spec.className || 'btn';
        button.dataset.popupAction = spec.action;
        button.textContent = spec.label;
        button.onclick = () => finish(spec.value);
        actions.appendChild(button);
        if (spec.primary) primary = button;
      }
      const cancel = buttons.find(spec => spec.action === 'cancel') || buttons[0];
      close.onclick = () => finish(cancel.value);
      dialog.addEventListener('cancel', event => { event.preventDefault(); finish(cancel.value); });
      document.body.appendChild(dialog);
      dialog.showModal();
      (primary || actions.lastElementChild)?.focus();
    });
  }

  globalThis.appAlert = message => popup('alert', message,
    [{label: '知道了', action: 'ok', value: undefined, primary: true, className: 'btn primary'}]);
  globalThis.appConfirm = (message, {ok = '确定', cancel = '取消', danger = false} = {}) => popup('confirm', message, [
    {label: cancel, action: 'cancel', value: false},
    {label: ok, action: 'ok', value: true, primary: true, className: danger ? 'btn danger' : 'btn primary'},
  ]);
  /** 浮动提示卡的容器：页面中央一列，卡片之间不重叠。 */
  globalThis.floatStack = () => {
    let stack = document.getElementById('float-stack');
    if (!stack) {
      stack = document.createElement('div');
      stack.id = 'float-stack';
      document.body.appendChild(stack);
    }
    return stack;
  };
})();
