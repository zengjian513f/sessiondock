'use strict';

// The conversation dialog and a bookmarked file URL read the same scoped log.
const TaskOutput = {
  number(task) { return '任务 ' + task.replace(/^task-/, ''); },

  mount(panel, {uid, agent, ref, task, command}, {close, back} = {}) {
    panel.classList.add('task-output-panel');
    panel.innerHTML = `<div class="modal-head task-output-head"><div><h2>后台输出</h2>
      <span class="task-output-number"></span></div></div>
      <pre class="task-output-command" hidden></pre>
      <div class="task-output-console"><div role="status">正在读取输出…</div>
      <pre class="task-output-text" tabindex="0" aria-label="日志输出" hidden></pre></div>
      <div class="task-output-controls"><label><input type="checkbox" checked>自动刷新</label>
      <button type="button" class="btn">刷新</button><a class="btn" download>下载完整日志</a></div>`;
    panel.querySelector('.task-output-number').textContent = this.number(task);
    const cmd = panel.querySelector('.task-output-command');
    if (command) {
      const lines = command.split(/\r?\n/);
      cmd.textContent = lines.length > 3 ? lines[0] : command;
      cmd.hidden = false;
      if (lines.length > 3) {
        const details = document.createElement('details');
        details.className = 'task-script';
        const summary = document.createElement('summary');
        summary.textContent = '完整命令（' + lines.length + ' 行）';
        const script = document.createElement('pre');
        script.className = 'task-output-command';
        script.textContent = command;
        details.append(summary, script);
        cmd.after(details);
      }
    }
    const head = panel.querySelector('.modal-head');
    if (back) {
      const button = document.createElement('button');
      button.type = 'button'; button.className = 'btn task-output-back'; button.textContent = '任务列表';
      button.onclick = back; head.append(button);
    }
    if (close) {
      const button = document.createElement('button');
      button.type = 'button'; button.className = 'modal-close'; button.textContent = '×';
      button.setAttribute('aria-label', '关闭'); button.title = '关闭';
      button.onclick = close; head.append(button);
    }
    const pre = panel.querySelector('.task-output-text'), status = panel.querySelector('[role=status]');
    const follow = panel.querySelector('input'), refresh = panel.querySelector('.task-output-controls button');
    const target = new URL('api/session/file', new URL('.', location.href));
    target.search = new URLSearchParams({uid, ref, agent: agent || '', download: '1'});
    panel.querySelector('a').href = target.href;
    const controller = new AbortController();
    let timer;
    function schedule() {
      clearTimeout(timer);
      if (follow.checked && !document.hidden && !controller.signal.aborted) timer = setTimeout(read, 3000);
    }
    async function read() {
      if (refresh.disabled || controller.signal.aborted) return;
      refresh.disabled = true;
      clearTimeout(timer);
      try {
        const response = await fetch(target, {cache: 'no-store', signal: controller.signal, headers: {Range: 'bytes=-65536'}});
        const range = response.headers.get('Content-Range') || '';
        const empty = response.status === 416 && range === 'bytes */0';
        if (!empty && response.status !== 206) {
          if (response.headers.get('Content-Type')?.includes('application/json')) {
            const body = await response.json();
            throw new Error(body.error || '无法读取日志');
          }
          await response.body?.cancel();
          throw new Error('请检查连接或登录状态');
        }
        const bytes = empty ? new Uint8Array() : new Uint8Array(await response.arrayBuffer());
        const clipped = !empty && !range.startsWith('bytes 0-');
        let first = 0;
        while (clipped && (bytes[first] & 0xc0) === 0x80) first++;
        const output = new TextDecoder().decode(bytes.subarray(first));
        const atEnd = pre.hidden || pre.scrollHeight - pre.scrollTop - pre.clientHeight <= 2;
        pre.hidden = !output;
        delete status.dataset.error;
        status.textContent = empty ? '暂时没有输出' : clipped ? '显示最近 64 KiB' : '';
        if (pre.textContent !== output) pre.textContent = output;
        if (atEnd) pre.scrollTop = pre.scrollHeight;
      } catch (error) {
        if (!controller.signal.aborted) {
          status.dataset.error = 'true';
          status.textContent = '读取失败 · ' + error.message;
        }
      } finally {
        refresh.disabled = false;
        schedule();
      }
    }
    refresh.onclick = read;
    follow.onchange = () => follow.checked ? read() : clearTimeout(timer);
    document.addEventListener('visibilitychange', () => {
      if (!document.hidden && follow.checked) read();
      else clearTimeout(timer);
    }, {signal: controller.signal});
    read();
    return () => { controller.abort(); clearTimeout(timer); };
  },
};
