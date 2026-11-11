'use strict';

/**
 * Reusable Web Voice Input component (compatible with SessionDock, MyTavern, etc.).
 * - Single click (< 220ms): open/close OpenRouter multimodal model selector
 * - Long press (>= 220ms): record microphone audio, stream context-aware transcription on release
 */
globalThis.VoiceInput = (() => {
  const MODELS = Object.freeze([
    Object.freeze({
      id: 'qwen/qwen3.8-omni-flash',
      label: 'Qwen 3.8 Omni Flash',
      short: 'Qwen 3.8 Omni',
      hint: '中文与中英混输推荐',
    }),
    Object.freeze({
      id: 'google/gemini-3.8-flash',
      label: 'Gemini 3.8 Flash',
      short: 'Gemini 3.8 Flash',
      hint: '长上下文与代码词精准',
    }),
    Object.freeze({
      id: 'openai/gpt-audio-mini',
      label: 'GPT Audio Mini',
      short: 'GPT Audio Mini',
      hint: 'OpenAI 原生语音小模型',
    }),
    Object.freeze({
      id: 'xiaomi/mimo-v2.6-flash',
      label: 'MiMo v2.6 Flash',
      short: 'MiMo v2.6 Flash',
      hint: '轻量低延迟备选',
    }),
  ]);

  const DEFAULT_MODEL = MODELS[0].id;
  const LONG_PRESS_MS = 220;
  const CANCEL_DRAG_PX = 42;
  const MOVE_SLOP_PX = 12;
  const MAX_RECORD_MS = 60000;
  const TARGET_SAMPLE_RATE = 16000;

  const MIC_SVG = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.85" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="9" y="2.5" width="6" height="11.5" rx="3"/><path d="M5.5 11a6.5 6.5 0 0 0 13 0"/><path d="M12 17.5v4"/><path d="M8.5 21.5h7"/></svg>`;

  const instances = new Set();

  function validModelId(id) {
    return MODELS.some(m => m.id === id) ? id : DEFAULT_MODEL;
  }

  function modelMeta(id) {
    return MODELS.find(m => m.id === id) || MODELS[0];
  }

  function bytesToBase64(bytes) {
    let binary = '';
    const chunk = 0x8000;
    for (let i = 0; i < bytes.length; i += chunk) {
      binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
    }
    return btoa(binary);
  }

  function encodeWav16k(floatChunks, inputRate) {
    let totalSamples = 0;
    for (const part of floatChunks) totalSamples += part.length;
    if (!totalSamples || !inputRate) {
      return {wavBytes: new Uint8Array(0), base64: '', durationMs: 0, rms: 0, peak: 0};
    }
    const merged = new Float32Array(totalSamples);
    let offset = 0;
    for (const part of floatChunks) {
      merged.set(part, offset);
      offset += part.length;
    }

    const ratio = inputRate / TARGET_SAMPLE_RATE;
    const outLength = Math.max(1, Math.round(merged.length / ratio));
    const pcm16 = new Int16Array(outLength);
    let sumSq = 0;
    let peak = 0;

    for (let i = 0; i < outLength; i++) {
      const start = Math.floor(i * ratio);
      const end = Math.min(merged.length, Math.max(start + 1, Math.floor((i + 1) * ratio)));
      let acc = 0;
      for (let j = start; j < end; j++) acc += merged[j];
      const sample = Math.max(-1, Math.min(1, acc / (end - start)));
      const abs = Math.abs(sample);
      if (abs > peak) peak = abs;
      sumSq += sample * sample;
      pcm16[i] = sample < 0 ? Math.round(sample * 0x8000) : Math.round(sample * 0x7fff);
    }

    const rms = Math.sqrt(sumSq / outLength);
    const durationMs = Math.round((outLength / TARGET_SAMPLE_RATE) * 1000);
    const dataByteLen = pcm16.length * 2;
    const buffer = new ArrayBuffer(44 + dataByteLen);
    const view = new DataView(buffer);
    const writeAscii = (pos, text) => {
      for (let i = 0; i < text.length; i++) view.setUint8(pos + i, text.charCodeAt(i));
    };

    writeAscii(0, 'RIFF');
    view.setUint32(4, 36 + dataByteLen, true);
    writeAscii(8, 'WAVE');
    writeAscii(12, 'fmt ');
    view.setUint32(16, 16, true);
    view.setUint16(20, 1, true); // PCM
    view.setUint16(22, 1, true); // Mono
    view.setUint32(24, TARGET_SAMPLE_RATE, true);
    view.setUint32(28, TARGET_SAMPLE_RATE * 2, true);
    view.setUint16(32, 2, true);
    view.setUint16(34, 16, true);
    writeAscii(36, 'data');
    view.setUint32(40, dataByteLen, true);

    const outBytes = new Uint8Array(buffer);
    outBytes.set(new Uint8Array(pcm16.buffer), 44);
    return {
      wavBytes: outBytes,
      base64: bytesToBase64(outBytes),
      durationMs,
      rms,
      peak,
    };
  }

  function formatElapsed(ms) {
    const sec = Math.max(0, Math.floor(ms / 1000));
    const m = Math.floor(sec / 60);
    const s = String(sec % 60).padStart(2, '0');
    return `${m}:${s}`;
  }

  function placePopover(popover, anchor, gap = 7) {
    if (!popover || !anchor || !anchor.isConnected) return;
    const anchorRect = anchor.getBoundingClientRect();
    const zoom = popover.currentCSSZoom || 1;
    const margin = 8;
    popover.style.maxWidth = `${Math.max(160, (window.innerWidth - margin * 2) / zoom)}px`;
    const box = popover.getBoundingClientRect();
    let x = anchorRect.right - box.width;
    x = Math.max(margin, Math.min(x, window.innerWidth - box.width - margin));
    let y = anchorRect.top - box.height - gap;
    if (y < margin && anchorRect.bottom + gap + box.height <= window.innerHeight - margin) {
      y = anchorRect.bottom + gap;
    } else {
      y = Math.max(margin, Math.min(y, window.innerHeight - box.height - margin));
    }
    popover.style.left = `${x / zoom}px`;
    popover.style.top = `${y / zoom}px`;
  }

  function popoverParent(anchor) {
    return anchor?.closest?.('dialog') || document.body;
  }

  function mountPopover(node, anchor) {
    const parent = popoverParent(anchor);
    if (node.parentElement !== parent) parent.appendChild(node);
    if (!node.matches(':popover-open')) {
      try { node.showPopover(); } catch { return false; }
    }
    placePopover(node, anchor);
    return true;
  }

  function hidePopoverSafe(node) {
    if (!node) return;
    try {
      if (node.matches(':popover-open')) node.hidePopover();
    } catch {}
  }

  function attach(options = {}) {
    const {
      input,
      buttonHost,
      beforeElement = null,
      endpoint = '/api/voice/transcribe',
      buttonClassName = 'btn voice-mic-btn',
      getModel = () => DEFAULT_MODEL,
      setModel = () => {},
      getContext = () => ({}),
      getRequestExtra = () => ({}),
      getHeaders = () => ({}),
      isDisabled = null,
      onInput = null,
    } = options;

    if (!input || !buttonHost) return null;

    const ac = new AbortController();
    const {signal} = ac;

    const button = document.createElement('button');
    button.type = 'button';
    button.className = buttonClassName;
    button.dataset.voiceInput = 'true';
    button.dataset.voiceState = 'idle';
    button.setAttribute('aria-haspopup', 'menu');
    button.setAttribute('aria-expanded', 'false');
    button.innerHTML = MIC_SVG;

    if (beforeElement && beforeElement.parentElement === buttonHost) {
      buttonHost.insertBefore(button, beforeElement);
    } else {
      buttonHost.appendChild(button);
    }

    const menu = document.createElement('div');
    menu.className = 'voice-model-menu';
    menu.setAttribute('popover', 'manual');
    menu.setAttribute('role', 'menu');
    menu.setAttribute('aria-label', '语音识别模型');

    const pill = document.createElement('div');
    pill.className = 'voice-status-pill';
    pill.setAttribute('popover', 'manual');
    pill.setAttribute('role', 'status');
    pill.setAttribute('aria-live', 'polite');

    let state = 'idle'; // 'idle' | 'recording' | 'transcribing'
    let menuOpen = false;
    let holdTimer = null;
    let pillTimer = null;
    let recordTickTimer = null;
    let maxRecordTimer = null;
    let activePointer = null;
    let cancelReady = false;
    let suppressNextClick = false;
    let savedSelection = null;
    let transcribeAbort = null;
    let recorderHandle = null;
    let recordStartedAt = 0;

    function currentModelId() {
      return validModelId(getModel());
    }

    function computeDisabled() {
      if (state === 'transcribing' || state === 'recording') return false;
      if (typeof isDisabled === 'function') return !!isDisabled();
      return !!(input.disabled || input.readOnly);
    }

    function syncButtonMeta() {
      const meta = modelMeta(currentModelId());
      button.disabled = computeDisabled();
      button.dataset.voiceModel = meta.id;
      button.dataset.voiceState = state;
      if (state === 'recording') {
        const msg = cancelReady ? '松开取消录音' : `正在录音（${meta.short}）· 松手转写`;
        button.title = msg;
        button.setAttribute('aria-label', msg);
      } else if (state === 'transcribing') {
        const msg = `正在语音识别（${meta.short}）· 单击取消`;
        button.title = msg;
        button.setAttribute('aria-label', msg);
      } else {
        const msg = `语音输入（${meta.label}）· 单击切换模型，长按录音`;
        button.title = msg;
        button.setAttribute('aria-label', msg);
      }
      if (menuOpen) renderMenuItems();
    }

    function renderMenuItems() {
      const selected = currentModelId();
      menu.innerHTML = '';
      const head = document.createElement('div');
      head.className = 'voice-model-menu-head';
      head.textContent = '语音模型 · 长按话筒录音';
      menu.appendChild(head);
      for (const item of MODELS) {
        const opt = document.createElement('button');
        opt.type = 'button';
        opt.className = 'voice-model-option';
        opt.setAttribute('role', 'menuitemradio');
        opt.dataset.modelId = item.id;
        const checked = item.id === selected;
        opt.setAttribute('aria-checked', String(checked));
        opt.innerHTML = `<span class="voice-model-check" aria-hidden="true">${checked ? '✓' : ''}</span>`
          + `<span class="voice-model-copy"><span class="voice-model-name"></span><span class="voice-model-hint"></span></span>`;
        opt.querySelector('.voice-model-name').textContent = item.label;
        opt.querySelector('.voice-model-hint').textContent = item.hint;
        opt.onclick = event => {
          event.stopPropagation();
          setModel(item.id);
          closeMenu();
          for (const inst of instances) inst.sync();
        };
        menu.appendChild(opt);
      }
    }

    function openMenu() {
      if (computeDisabled() || state !== 'idle') return;
      for (const inst of instances) {
        if (inst !== api) inst.closeMenu();
      }
      hidePill();
      renderMenuItems();
      menuOpen = true;
      button.classList.add('on');
      button.setAttribute('aria-expanded', 'true');
      mountPopover(menu, button);
    }

    function closeMenu() {
      if (!menuOpen) return;
      menuOpen = false;
      button.classList.remove('on');
      button.setAttribute('aria-expanded', 'false');
      hidePopoverSafe(menu);
    }

    function toggleMenu() {
      if (menuOpen) closeMenu();
      else openMenu();
    }

    function showPill(contentNodeOrText, {tone = 'normal', durationMs = 0} = {}) {
      clearTimeout(pillTimer);
      pillTimer = null;
      pill.dataset.tone = tone;
      pill.innerHTML = '';
      if (typeof contentNodeOrText === 'string') {
        const span = document.createElement('span');
        span.className = 'voice-status-text';
        span.textContent = contentNodeOrText;
        pill.appendChild(span);
      } else {
        pill.appendChild(contentNodeOrText);
      }
      mountPopover(pill, button);
      if (durationMs > 0) {
        pillTimer = setTimeout(() => hidePill(), durationMs);
      }
    }

    function hidePill() {
      clearTimeout(pillTimer);
      pillTimer = null;
      hidePopoverSafe(pill);
    }

    function renderRecordingPill(level = 0) {
      const frag = document.createDocumentFragment();
      if (!cancelReady) {
        const dot = document.createElement('span');
        dot.className = 'voice-rec-dot';
        const bars = document.createElement('span');
        bars.className = 'voice-bars';
        bars.setAttribute('aria-hidden', 'true');
        const weights = [0.65, 1.0, 0.85, 0.55];
        for (const w of weights) {
          const bar = document.createElement('i');
          const h = Math.round(4 + Math.min(1, level * 6.5 * w) * 10);
          bar.style.height = `${h}px`;
          bars.appendChild(bar);
        }
        frag.append(dot, bars);
      }
      const text = document.createElement('span');
      text.className = 'voice-status-text';
      const elapsed = formatElapsed(Date.now() - recordStartedAt);
      text.textContent = cancelReady
        ? `松开手指取消录音 (${elapsed})`
        : `正在录音 ${elapsed} · 松手转写 / 上滑取消`;
      frag.appendChild(text);
      showPill(frag, {tone: cancelReady ? 'cancel' : 'normal'});
    }

    function snapshotInputSelection() {
      try {
        const start = typeof input.selectionStart === 'number' ? input.selectionStart : input.value.length;
        const end = typeof input.selectionEnd === 'number' ? input.selectionEnd : start;
        return {start, end, before: input.value.slice(0, start), after: input.value.slice(end)};
      } catch {
        const val = String(input.value || '');
        return {start: val.length, end: val.length, before: val, after: ''};
      }
    }

    async function openAudioRecorder(onLevel) {
      if (typeof globalThis.__VOICE_INPUT_TEST_RECORDER__ === 'function') {
        return await globalThis.__VOICE_INPUT_TEST_RECORDER__(onLevel);
      }
      if (!navigator.mediaDevices?.getUserMedia) {
        throw new Error('当前浏览器环境不支持麦克风录音（需 HTTPS 或 localhost）');
      }
      const stream = await navigator.mediaDevices.getUserMedia({
        audio: {
          channelCount: 1,
          echoCancellation: true,
          noiseSuppression: true,
          autoGainControl: true,
        },
      });
      const AudioCtx = globalThis.AudioContext || globalThis.webkitAudioContext;
      const ctx = new AudioCtx();
      if (ctx.state === 'suspended') {
        try { await ctx.resume(); } catch {}
      }
      const source = ctx.createMediaStreamSource(stream);
      const processor = ctx.createScriptProcessor(2048, 1, 1);
      const gain = ctx.createGain();
      gain.gain.value = 0;
      const chunks = [];
      processor.onaudioprocess = event => {
        const channel = event.inputBuffer.getChannelData(0);
        const copy = new Float32Array(channel.length);
        copy.set(channel);
        chunks.push(copy);
        let sq = 0;
        for (let i = 0; i < copy.length; i += 4) sq += copy[i] * copy[i];
        const rms = Math.sqrt(sq / Math.max(1, Math.ceil(copy.length / 4)));
        onLevel?.(rms);
      };
      source.connect(processor);
      processor.connect(gain);
      gain.connect(ctx.destination);

      let closed = false;
      const cleanup = async () => {
        if (closed) return;
        closed = true;
        try { processor.disconnect(); } catch {}
        try { source.disconnect(); } catch {}
        try { gain.disconnect(); } catch {}
        for (const track of stream.getTracks()) {
          try { track.stop(); } catch {}
        }
        try { await ctx.close(); } catch {}
      };

      return {
        async stop() {
          const sampleRate = ctx.sampleRate || 48000;
          await cleanup();
          return encodeWav16k(chunks, sampleRate);
        },
        async cancel() {
          chunks.length = 0;
          await cleanup();
        },
      };
    }

    function clearRecordingTimers() {
      clearInterval(recordTickTimer);
      recordTickTimer = null;
      clearTimeout(maxRecordTimer);
      maxRecordTimer = null;
    }

    async function startRecording() {
      if (state !== 'idle' || computeDisabled()) return;
      closeMenu();
      savedSelection = snapshotInputSelection();
      cancelReady = false;
      button.dataset.cancelReady = 'false';
      state = 'recording';
      recordStartedAt = Date.now();
      syncButtonMeta();
      showPill('正在请求麦克风…');

      const promptStart = Date.now();
      let latestLevel = 0;
      let handle;
      try {
        handle = await openAudioRecorder(rms => { latestLevel = rms; });
      } catch (error) {
        state = 'idle';
        activePointer = null;
        syncButtonMeta();
        const denied = error?.name === 'NotAllowedError' || error?.name === 'PermissionDeniedError';
        showPill(denied ? '麦克风权限被拒绝，请在浏览器地址栏允许访问' : `无法启动麦克风：${error?.message || error}`, {
          tone: 'error',
          durationMs: 3200,
        });
        return;
      }

      // If pointer was already released while waiting on browser's permission dialog:
      if (state !== 'recording' || !activePointer) {
        const waitedForPermission = Date.now() - promptStart > 350;
        await handle.cancel();
        state = 'idle';
        syncButtonMeta();
        if (waitedForPermission) {
          showPill('麦克风已就绪，请再次长按话筒说话', {durationMs: 2200});
        } else {
          hidePill();
        }
        return;
      }

      recorderHandle = handle;
      recordStartedAt = Date.now();
      renderRecordingPill(0);
      recordTickTimer = setInterval(() => {
        if (state === 'recording') renderRecordingPill(latestLevel);
      }, 90);
      maxRecordTimer = setTimeout(() => {
        if (state === 'recording') void finishRecording(false);
      }, MAX_RECORD_MS);
    }

    async function finishRecording(cancelled) {
      clearTimeout(holdTimer);
      holdTimer = null;
      clearRecordingTimers();
      activePointer = null;
      const wasCancelReady = cancelReady;
      cancelReady = false;
      button.dataset.cancelReady = 'false';

      if (state !== 'recording') return;
      const handle = recorderHandle;
      recorderHandle = null;

      if (!handle) {
        state = 'idle';
        syncButtonMeta();
        hidePill();
        return;
      }

      if (cancelled || wasCancelReady) {
        state = 'idle';
        syncButtonMeta();
        await handle.cancel();
        showPill('已取消录音', {tone: 'cancel', durationMs: 1100});
        return;
      }

      let encoded;
      try {
        encoded = await handle.stop();
      } catch (error) {
        state = 'idle';
        syncButtonMeta();
        showPill(`录音处理失败：${error?.message || error}`, {tone: 'error', durationMs: 2800});
        return;
      }

      if (!encoded.base64 || encoded.durationMs < 180 || (encoded.rms < 0.0008 && encoded.peak < 0.004)) {
        state = 'idle';
        syncButtonMeta();
        showPill('未检测到语音', {tone: 'cancel', durationMs: 1500});
        return;
      }

      await streamTranscription(encoded);
    }

    function applyTranscriptText(sel, nextText) {
      let text = nextText;
      const maxLen = Number(input.maxLength);
      if (Number.isFinite(maxLen) && maxLen > 0) {
        const otherLen = input.value.length - (sel.end - sel.start);
        const budget = Math.max(0, maxLen - otherLen);
        if (text.length > budget) text = text.slice(0, budget);
      }
      if (typeof input.setRangeText === 'function') {
        input.setRangeText(text, sel.start, sel.end, 'end');
      } else {
        input.value = input.value.slice(0, sel.start) + text + input.value.slice(sel.end);
      }
      sel.end = sel.start + text.length;
      input.dispatchEvent(new Event('input', {bubbles: true}));
      onInput?.(text, false);
    }

    async function streamTranscription(encoded) {
      const model = currentModelId();
      const meta = modelMeta(model);
      const sel = savedSelection
        ? {start: Math.min(savedSelection.start, input.value.length), end: Math.min(savedSelection.end, input.value.length)}
        : {start: input.value.length, end: input.value.length};
      const rawContext = getContext() || {};
      const context = {
        title: String(rawContext.title || ''),
        messages: Array.isArray(rawContext.messages) ? rawContext.messages : [],
        draft_before: savedSelection?.before ?? input.value.slice(0, sel.start),
        draft_after: savedSelection?.after ?? input.value.slice(sel.end),
      };
      const extra = getRequestExtra() || {};

      state = 'transcribing';
      button.setAttribute('aria-busy', 'true');
      syncButtonMeta();
      showPill(`正在识别（${meta.short}）… 单击可取消`);

      const reqAbort = new AbortController();
      transcribeAbort = reqAbort;
      let streamed = '';

      try {
        const rawEndpoint = typeof endpoint === 'function' ? endpoint() : endpoint;
        const resolvedUrl = /^https?:\/\//i.test(String(rawEndpoint))
          ? String(rawEndpoint)
          : new URL(String(rawEndpoint).replace(/^\//, ''), new URL('.', location.href)).href;
        const response = await fetch(resolvedUrl, {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            'Accept': 'text/event-stream',
            ...(getHeaders() || {}),
          },
          body: JSON.stringify({
            ...extra,
            model,
            audio: encoded.base64,
            format: 'wav',
            context,
          }),
          signal: reqAbort.signal,
        });

        const ctype = response.headers.get('content-type') || '';
        if (!response.ok) {
          let errMsg = `HTTP ${response.status}`;
          if (ctype.includes('application/json')) {
            const payload = await response.json().catch(() => null);
            if (payload?.error) errMsg = payload.error;
          } else {
            const txt = await response.text().catch(() => '');
            if (txt.trim()) errMsg = txt.trim().slice(0, 140);
          }
          throw new Error(errMsg);
        }

        if (!ctype.includes('text/event-stream') || !response.body) {
          const payload = await response.json().catch(() => ({}));
          if (payload?.error) throw new Error(payload.error);
          const full = String(payload?.text || '');
          if (full) {
            streamed = full;
            applyTranscriptText(sel, streamed);
          }
        } else {
          const reader = response.body.getReader();
          const decoder = new TextDecoder();
          let buffer = '';
          let streamDone = false;
          while (!streamDone) {
            const {value, done} = await reader.read();
            if (done) break;
            buffer += decoder.decode(value, {stream: true});
            let nl;
            while ((nl = buffer.indexOf('\n')) >= 0) {
              const line = buffer.slice(0, nl).trim();
              buffer = buffer.slice(nl + 1);
              if (!line.startsWith('data:')) continue;
              const raw = line.slice(5).trim();
              if (!raw || raw === '[DONE]') continue;
              let event;
              try { event = JSON.parse(raw); } catch { continue; }
              if (event.error) throw new Error(event.error);
              if (typeof event.delta === 'string' && event.delta) {
                streamed += event.delta;
                applyTranscriptText(sel, streamed);
              }
              if (event.done) {
                if (typeof event.text === 'string' && event.text !== streamed) {
                  streamed = event.text;
                  applyTranscriptText(sel, streamed);
                }
                streamDone = true;
                break;
              }
            }
          }
        }

        state = 'idle';
        button.removeAttribute('aria-busy');
        syncButtonMeta();
        if (!streamed.trim()) {
          showPill('未识别到说话内容', {tone: 'cancel', durationMs: 1500});
        } else {
          hidePill();
          try {
            input.focus({preventScroll: true});
            input.setSelectionRange(sel.end, sel.end);
          } catch {}
          onInput?.(streamed, true);
        }
      } catch (error) {
        state = 'idle';
        button.removeAttribute('aria-busy');
        syncButtonMeta();
        if (reqAbort.signal.aborted) {
          showPill('已取消语音识别', {tone: 'cancel', durationMs: 1200});
        } else {
          showPill(`语音识别失败：${error?.message || error}`, {tone: 'error', durationMs: 3400});
        }
      } finally {
        if (transcribeAbort === reqAbort) transcribeAbort = null;
      }
    }

    function cancelActiveWork() {
      clearTimeout(holdTimer);
      holdTimer = null;
      if (state === 'recording') {
        void finishRecording(true);
      } else if (state === 'transcribing' && transcribeAbort) {
        transcribeAbort.abort();
      }
    }

    button.addEventListener('mousedown', event => {
      // Keep focus and caret selection inside the composer input when pressing the mic button.
      if (event.button === 0) event.preventDefault();
    }, {signal});

    button.addEventListener('contextmenu', event => {
      event.preventDefault();
    }, {signal});

    button.addEventListener('pointerdown', event => {
      if (!event.isPrimary || (event.pointerType === 'mouse' && event.button !== 0)) return;
      if (computeDisabled()) return;

      if (state === 'transcribing') {
        suppressNextClick = true;
        transcribeAbort?.abort();
        return;
      }
      if (state !== 'idle') return;

      savedSelection = snapshotInputSelection();
      activePointer = {
        id: event.pointerId,
        startX: event.clientX,
        startY: event.clientY,
        longPressed: false,
        movedTooFar: false,
      };
      try { button.setPointerCapture(event.pointerId); } catch {}
      clearTimeout(holdTimer);
      holdTimer = setTimeout(() => {
        holdTimer = null;
        if (!activePointer || activePointer.movedTooFar) return;
        activePointer.longPressed = true;
        suppressNextClick = true;
        void startRecording();
      }, LONG_PRESS_MS);
    }, {signal});

    button.addEventListener('pointermove', event => {
      if (!activePointer || event.pointerId !== activePointer.id) return;
      const dx = event.clientX - activePointer.startX;
      const dy = event.clientY - activePointer.startY;
      if (!activePointer.longPressed) {
        if (Math.hypot(dx, dy) > MOVE_SLOP_PX) {
          activePointer.movedTooFar = true;
          clearTimeout(holdTimer);
          holdTimer = null;
        }
        return;
      }
      if (state === 'recording') {
        const rect = button.getBoundingClientRect();
        const nextCancel = event.clientY < rect.top - CANCEL_DRAG_PX;
        if (nextCancel !== cancelReady) {
          cancelReady = nextCancel;
          button.dataset.cancelReady = String(cancelReady);
          syncButtonMeta();
          renderRecordingPill(0);
        }
      }
    }, {signal});

    const onPointerFinish = (event, cancelled) => {
      if (!activePointer || event.pointerId !== activePointer.id) return;
      const pointer = activePointer;
      clearTimeout(holdTimer);
      holdTimer = null;
      try {
        if (button.hasPointerCapture(event.pointerId)) button.releasePointerCapture(event.pointerId);
      } catch {}

      if (pointer.longPressed || state === 'recording') {
        suppressNextClick = true;
        void finishRecording(cancelled);
        return;
      }

      activePointer = null;
      if (!cancelled && !pointer.movedTooFar) {
        suppressNextClick = true;
        toggleMenu();
      }
    };

    button.addEventListener('pointerup', event => onPointerFinish(event, false), {signal});
    button.addEventListener('pointercancel', event => onPointerFinish(event, true), {signal});

    button.addEventListener('click', event => {
      if (suppressNextClick) {
        suppressNextClick = false;
        event.preventDefault();
        event.stopPropagation();
        return;
      }
      // Keyboard Enter / Space activation fallback.
      if (state === 'transcribing') {
        transcribeAbort?.abort();
      } else if (state === 'idle' && !computeDisabled()) {
        toggleMenu();
      }
    }, {signal});

    document.addEventListener('pointerdown', event => {
      if (!menuOpen) return;
      if (button.contains(event.target) || menu.contains(event.target)) return;
      closeMenu();
    }, {signal, capture: true});

    document.addEventListener('keydown', event => {
      if (event.key === 'Escape') {
        if (menuOpen) {
          event.preventDefault();
          event.stopPropagation();
          closeMenu();
          return;
        }
        if (state === 'recording' || state === 'transcribing') {
          event.preventDefault();
          event.stopPropagation();
          cancelActiveWork();
        }
      }
    }, {signal, capture: true});

    window.addEventListener('resize', () => {
      if (menuOpen) placePopover(menu, button);
      if (pill.matches(':popover-open')) placePopover(pill, button);
    }, {signal});

    window.addEventListener('blur', () => {
      if (state === 'recording') void finishRecording(true);
    }, {signal});

    const hostDialog = button.closest?.('dialog');
    if (hostDialog) {
      hostDialog.addEventListener('close', () => {
        closeMenu();
        cancelActiveWork();
        hidePill();
      }, {signal});
    }

    const inputObserver = new MutationObserver(() => syncButtonMeta());
    inputObserver.observe(input, {attributes: true, attributeFilter: ['disabled', 'readonly']});

    const api = {
      button,
      menu,
      pill,
      sync: syncButtonMeta,
      closeMenu,
      destroy() {
        instances.delete(api);
        cancelActiveWork();
        closeMenu();
        hidePill();
        inputObserver.disconnect();
        ac.abort();
        menu.remove();
        pill.remove();
        button.remove();
      },
    };

    instances.add(api);
    syncButtonMeta();
    return api;
  }

  return Object.freeze({
    MODELS,
    DEFAULT_MODEL,
    encodeWav16k,
    attach,
  });
})();
