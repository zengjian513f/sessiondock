'use strict';

/**
 * Reusable Web Voice Input component (compatible with SessionDock, MyTavern, etc.).
 * - 3 Providers: OpenAI, Google AI Studio, OpenRouter (each with dedicated models)
 * - Single click (< 220ms): open/close 3-provider model selector
 * - Long press (>= 220ms):
 *   - Real-time models (`realtime: true`): stream 16kHz PCM over WebSocket, write text into
 *     <textarea> live as the user speaks ("边说话边出字"), with dedicated live HUD card &
 *     clean rollback on slide-up / Escape cancel
 *   - HTTP/SSE models (`realtime: false`): record WAV and stream context-aware transcription on release
 */
globalThis.VoiceInput = (() => {
  const PROVIDERS = Object.freeze([
    Object.freeze({id: 'openai', label: 'OpenAI', short: 'OpenAI'}),
    Object.freeze({id: 'google', label: 'Google AI Studio', short: 'Google'}),
    Object.freeze({id: 'openrouter', label: 'OpenRouter', short: 'OpenRouter'}),
  ]);

  const MODELS = Object.freeze([
    // OpenAI
    Object.freeze({
      id: 'openai:gpt-live-transcribe',
      provider: 'openai',
      realtime: true,
      label: 'GPT Live Transcribe',
      short: 'GPT Live',
      hint: '边说边出字 · OpenAI 原生实时转写',
    }),
    Object.freeze({
      id: 'openai:gpt-4o-transcribe',
      provider: 'openai',
      realtime: false,
      label: 'GPT-4o Transcribe',
      short: 'GPT-4o Transcribe',
      hint: '高精语音识别 · 松手流式转写',
    }),
    Object.freeze({
      id: 'openai:gpt-4o-mini-transcribe',
      provider: 'openai',
      realtime: false,
      label: 'GPT-4o Mini Transcribe',
      short: 'GPT-4o Mini',
      hint: '轻量低延迟 · 松手流式转写',
    }),
    // Google AI Studio
    Object.freeze({
      id: 'google:gemini-3.5-transcribe-live',
      provider: 'google',
      realtime: true,
      label: 'Gemini 3.5 Transcribe Live',
      short: 'Gemini Live',
      hint: '边说边出字 · Gemini Live 双向流',
    }),
    Object.freeze({
      id: 'google:gemini-3.8-flash',
      provider: 'google',
      realtime: false,
      label: 'Gemini 3.8 Flash',
      short: 'Gemini 3.8 Flash',
      hint: '上下文热词与代码术语校准',
    }),
    Object.freeze({
      id: 'google:gemini-4-carbon',
      provider: 'google',
      realtime: false,
      label: 'Gemini 4 Carbon',
      short: 'Gemini 4 Carbon',
      hint: '旗舰多模态复杂语境理解',
    }),
    // OpenRouter (retaining legacy bare aliases for backward compatibility)
    Object.freeze({
      id: 'openrouter:qwen/qwen3.8-omni-flash',
      legacyAlias: 'qwen/qwen3.8-omni-flash',
      provider: 'openrouter',
      realtime: false,
      label: 'Qwen 3.8 Omni Flash',
      short: 'Qwen 3.8 Omni',
      hint: '中文与中英混输推荐',
    }),
    Object.freeze({
      id: 'openrouter:google/gemini-3.8-flash',
      legacyAlias: 'google/gemini-3.8-flash',
      provider: 'openrouter',
      realtime: false,
      label: 'Gemini 3.8 Flash (OR)',
      short: 'Gemini 3.8 OR',
      hint: 'OpenRouter 聚合多模态转写',
    }),
    Object.freeze({
      id: 'openrouter:openai/gpt-audio-mini',
      legacyAlias: 'openai/gpt-audio-mini',
      provider: 'openrouter',
      realtime: false,
      label: 'GPT Audio Mini',
      short: 'GPT Audio Mini',
      hint: 'OpenRouter 聚合原生语音模型',
    }),
    Object.freeze({
      id: 'openrouter:xiaomi/mimo-v2.6-flash',
      legacyAlias: 'xiaomi/mimo-v2.6-flash',
      provider: 'openrouter',
      realtime: false,
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
  const PCM_FRAME_SAMPLES = 1600; // 100ms @ 16kHz

  const MIC_SVG = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.85" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="9" y="2.5" width="6" height="11.5" rx="3"/><path d="M5.5 11a6.5 6.5 0 0 0 13 0"/><path d="M12 17.5v4"/><path d="M8.5 21.5h7"/></svg>`;

  const instances = new Set();

  function findModelSpec(id) {
    const raw = String(id || '').trim();
    if (!raw) return null;
    return MODELS.find(m => m.id === raw || m.legacyAlias === raw) || null;
  }

  function resolveAvailableProviders(getProviders) {
    if (typeof getProviders === 'function') {
      const list = getProviders();
      if (Array.isArray(list) && list.length > 0) {
        const valid = list
          .map(p => String(p || '').trim().toLowerCase())
          .filter(p => PROVIDERS.some(pr => pr.id === p));
        if (valid.length > 0) return valid;
      }
    }
    return PROVIDERS.map(p => p.id);
  }

  function validModelId(id, availableProviders = null) {
    const allowed = Array.isArray(availableProviders) && availableProviders.length
      ? availableProviders
      : PROVIDERS.map(p => p.id);
    const spec = findModelSpec(id);
    if (spec && allowed.includes(spec.provider)) return spec.id;
    const fallback = MODELS.find(m => allowed.includes(m.provider)) || MODELS[0];
    return fallback.id;
  }

  function modelMeta(id, availableProviders = null) {
    const resolved = validModelId(id, availableProviders);
    return findModelSpec(resolved) || MODELS[0];
  }

  function providerMeta(id) {
    return PROVIDERS.find(p => p.id === id) || PROVIDERS[0];
  }

  function bytesToBase64(bytes) {
    let binary = '';
    const chunk = 0x8000;
    for (let i = 0; i < bytes.length; i += chunk) {
      binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
    }
    return btoa(binary);
  }

  function downsampleFloatToPcm16(floatBuf, inputRate, targetRate = TARGET_SAMPLE_RATE) {
    if (!floatBuf?.length || !inputRate) return new Int16Array(0);
    if (inputRate === targetRate) {
      const out = new Int16Array(floatBuf.length);
      for (let i = 0; i < floatBuf.length; i++) {
        const s = Math.max(-1, Math.min(1, floatBuf[i]));
        out[i] = s < 0 ? Math.round(s * 0x8000) : Math.round(s * 0x7fff);
      }
      return out;
    }
    const ratio = inputRate / targetRate;
    const outLength = Math.max(1, Math.round(floatBuf.length / ratio));
    const out = new Int16Array(outLength);
    for (let i = 0; i < outLength; i++) {
      const start = Math.floor(i * ratio);
      const end = Math.min(floatBuf.length, Math.max(start + 1, Math.floor((i + 1) * ratio)));
      let acc = 0;
      for (let j = start; j < end; j++) acc += floatBuf[j];
      const sample = Math.max(-1, Math.min(1, acc / (end - start)));
      out[i] = sample < 0 ? Math.round(sample * 0x8000) : Math.round(sample * 0x7fff);
    }
    return out;
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

  function placePopover(popover, anchor, gap = 8) {
    if (!popover || !anchor || !anchor.isConnected) return;
    const anchorRect = anchor.getBoundingClientRect();
    const zoom = popover.currentCSSZoom || 1;
    const margin = 8;
    popover.style.maxWidth = `${Math.max(180, (window.innerWidth - margin * 2) / zoom)}px`;
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

  function resolveHttpUrl(rawEndpoint) {
    const raw = String(typeof rawEndpoint === 'function' ? rawEndpoint() : rawEndpoint);
    return /^https?:\/\//i.test(raw)
      ? raw
      : new URL(raw.replace(/^\//, ''), new URL('.', location.href)).href;
  }

  function resolveWsUrl(rawEndpoint, paramsObj) {
    const raw = String(typeof rawEndpoint === 'function' ? rawEndpoint() : rawEndpoint);
    const baseHref = /^wss?:\/\//i.test(raw)
      ? raw
      : new URL(raw.replace(/^\//, ''), new URL('.', location.href)).href.replace(/^http/i, 'ws');
    const url = new URL(baseHref);
    for (const [k, v] of Object.entries(paramsObj || {})) {
      if (v != null && String(v) !== '') url.searchParams.set(k, String(v));
    }
    return url.href;
  }

  function attach(options = {}) {
    const {
      input,
      buttonHost,
      beforeElement = null,
      endpoint = '/api/voice/transcribe',
      realtimeEndpoint = '/api/voice/realtime',
      buttonClassName = 'btn voice-mic-btn',
      getModel = () => DEFAULT_MODEL,
      setModel = () => {},
      getProviders = null,
      getContext = () => ({}),
      getRequestExtra = () => ({}),
      getRealtimeParams = null,
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
    menu.setAttribute('role', 'dialog');
    menu.setAttribute('aria-label', '语音识别服务与模型');

    const pill = document.createElement('div');
    pill.className = 'voice-status-pill';
    pill.setAttribute('popover', 'manual');
    pill.setAttribute('role', 'status');
    pill.setAttribute('aria-live', 'polite');

    let state = 'idle'; // 'idle' | 'recording' | 'transcribing'
    let menuOpen = false;
    let activeProviderTab = null;
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
    let realtimeSession = null;
    let recordStartedAt = 0;
    let latestAudioLevel = 0;

    function availableProviders() {
      return resolveAvailableProviders(getProviders);
    }

    function currentModelId() {
      return validModelId(getModel(), availableProviders());
    }

    function computeDisabled() {
      if (state === 'transcribing' || state === 'recording') return false;
      if (typeof isDisabled === 'function') return !!isDisabled();
      return !!(input.disabled || input.readOnly);
    }

    function syncButtonMeta() {
      const providers = availableProviders();
      const meta = modelMeta(getModel(), providers);
      const prov = providerMeta(meta.provider);
      button.disabled = computeDisabled();
      button.dataset.voiceModel = meta.id;
      button.dataset.voiceProvider = meta.provider;
      button.dataset.voiceRealtime = String(!!meta.realtime);
      button.dataset.voiceState = state;
      if (state === 'recording') {
        const msg = cancelReady
          ? (meta.realtime ? '松开取消并撤回输入' : '松开取消录音')
          : (meta.realtime ? `实时听写（${meta.short}）· 边说边出字，松手完成` : `正在录音（${meta.short}）· 松手转写`);
        button.title = msg;
        button.setAttribute('aria-label', msg);
      } else if (state === 'transcribing') {
        const msg = meta.realtime
          ? `正在收尾校准（${meta.short}）· 单击立即结束`
          : `正在语音识别（${meta.short}）· 单击取消`;
        button.title = msg;
        button.setAttribute('aria-label', msg);
      } else {
        const modeTag = meta.realtime ? '实时边说边出字' : '松手流式转写';
        const msg = `语音输入（${prov.short} · ${meta.label} · ${modeTag}）· 单击切换模型，长按说话`;
        button.title = msg;
        button.setAttribute('aria-label', msg);
      }
      if (menuOpen) renderMenuItems();
    }

    function renderMenuItems() {
      const providers = availableProviders();
      const selected = currentModelId();
      const selectedMeta = modelMeta(selected, providers);
      if (!activeProviderTab || !PROVIDERS.some(p => p.id === activeProviderTab)) {
        activeProviderTab = selectedMeta.provider;
      }

      menu.innerHTML = '';

      const head = document.createElement('div');
      head.className = 'voice-model-menu-head';
      const headTitle = document.createElement('span');
      headTitle.textContent = '语音输入 · 单击选模型 / 长按说话';
      head.appendChild(headTitle);
      menu.appendChild(head);

      const tabs = document.createElement('div');
      tabs.className = 'voice-provider-tabs';
      tabs.setAttribute('role', 'tablist');
      tabs.setAttribute('aria-label', '语音服务提供商');

      for (const prov of PROVIDERS) {
        const enabled = providers.includes(prov.id);
        const tab = document.createElement('button');
        tab.type = 'button';
        tab.className = 'voice-provider-tab';
        tab.setAttribute('role', 'tab');
        tab.dataset.providerId = prov.id;
        const isCurrent = prov.id === activeProviderTab;
        tab.setAttribute('aria-selected', String(isCurrent));
        if (!enabled) tab.dataset.unconfigured = 'true';
        const hasSelectedModel = selectedMeta.provider === prov.id;
        tab.innerHTML = `<span class="voice-provider-tab-label"></span>`
          + (hasSelectedModel ? `<i class="voice-provider-active-dot" aria-hidden="true"></i>` : '');
        tab.querySelector('.voice-provider-tab-label').textContent = prov.short;
        tab.onclick = event => {
          event.stopPropagation();
          activeProviderTab = prov.id;
          renderMenuItems();
          placePopover(menu, button);
        };
        tabs.appendChild(tab);
      }
      menu.appendChild(tabs);

      const list = document.createElement('div');
      list.className = 'voice-model-list';
      list.setAttribute('role', 'menu');

      const providerEnabled = providers.includes(activeProviderTab);
      if (!providerEnabled) {
        const note = document.createElement('div');
        note.className = 'voice-provider-note';
        note.textContent = `当前服务端尚未配置 ${providerMeta(activeProviderTab).label} Key`;
        list.appendChild(note);
      }

      const providerModels = MODELS.filter(m => m.provider === activeProviderTab);
      for (const item of providerModels) {
        const opt = document.createElement('button');
        opt.type = 'button';
        opt.className = 'voice-model-option';
        opt.setAttribute('role', 'menuitemradio');
        opt.dataset.modelId = item.id;
        opt.dataset.realtime = String(!!item.realtime);
        const checked = item.id === selected;
        opt.setAttribute('aria-checked', String(checked));
        if (!providerEnabled) {
          opt.disabled = true;
          opt.setAttribute('aria-disabled', 'true');
        }
        opt.innerHTML = `<span class="voice-model-check" aria-hidden="true">${checked ? '✓' : ''}</span>`
          + `<span class="voice-model-copy">`
          + `<span class="voice-model-row"><span class="voice-model-name"></span>${item.realtime ? '<span class="voice-realtime-badge">实时</span>' : ''}</span>`
          + `<span class="voice-model-hint"></span>`
          + `</span>`;
        opt.querySelector('.voice-model-name').textContent = item.label;
        opt.querySelector('.voice-model-hint').textContent = item.hint;
        opt.onclick = event => {
          event.stopPropagation();
          if (!providerEnabled) return;
          setModel(item.id);
          closeMenu();
          for (const inst of instances) inst.sync();
        };
        list.appendChild(opt);
      }

      menu.appendChild(list);
    }

    function openMenu() {
      if (computeDisabled() || state !== 'idle') return;
      for (const inst of instances) {
        if (inst !== api) inst.closeMenu();
      }
      hidePill();
      activeProviderTab = modelMeta(currentModelId(), availableProviders()).provider;
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

    function showPill(contentNodeOrText, {tone = 'normal', mode = 'compact', durationMs = 0} = {}) {
      clearTimeout(pillTimer);
      pillTimer = null;
      pill.dataset.tone = tone;
      pill.dataset.mode = mode;
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
      delete pill.dataset.mode;
      hidePopoverSafe(pill);
    }

    function createMeterBars(level = 0, weights = [0.65, 1.0, 0.85, 0.55], className = 'voice-bars') {
      const bars = document.createElement('span');
      bars.className = className;
      bars.setAttribute('aria-hidden', 'true');
      for (const w of weights) {
        const bar = document.createElement('i');
        const h = Math.round(4 + Math.min(1, level * 6.8 * w) * 10);
        bar.style.height = `${h}px`;
        bars.appendChild(bar);
      }
      return bars;
    }

    function renderBatchRecordingPill(level = 0) {
      const meta = modelMeta(currentModelId(), availableProviders());
      const frag = document.createDocumentFragment();
      if (!cancelReady) {
        const dot = document.createElement('span');
        dot.className = 'voice-rec-dot';
        frag.append(dot, createMeterBars(level));
      }
      const text = document.createElement('span');
      text.className = 'voice-status-text';
      const elapsed = formatElapsed(Date.now() - recordStartedAt);
      text.textContent = cancelReady
        ? `松开手指取消录音 (${elapsed})`
        : `正在录音 ${elapsed}（${meta.short}）· 松手转写 / 上滑取消`;
      frag.appendChild(text);
      showPill(frag, {tone: cancelReady ? 'cancel' : 'normal', mode: 'compact'});
    }

    function renderRealtimeCard(rt, level = 0) {
      if (!rt) return;
      const card = document.createElement('div');
      card.className = 'voice-live-card';
      if (rt.finalizing) card.dataset.phase = 'finalizing';
      else if (cancelReady) card.dataset.phase = 'cancel';
      else card.dataset.phase = rt.ready ? 'streaming' : 'connecting';

      const head = document.createElement('div');
      head.className = 'voice-live-head';

      const left = document.createElement('div');
      left.className = 'voice-live-title';
      const dot = document.createElement('span');
      dot.className = 'voice-live-dot';
      const badge = document.createElement('span');
      badge.className = 'voice-live-badge';
      if (cancelReady) badge.textContent = '松手撤回';
      else if (rt.finalizing) badge.textContent = '正在收尾';
      else badge.textContent = '实时听写';

      const modelSpan = document.createElement('span');
      modelSpan.className = 'voice-live-model';
      modelSpan.textContent = rt.meta.short;
      left.append(dot, badge, modelSpan);

      const right = document.createElement('div');
      right.className = 'voice-live-right';
      if (!cancelReady && !rt.finalizing) {
        right.appendChild(createMeterBars(level, [0.55, 0.9, 1.0, 0.8, 0.5], 'voice-bars voice-live-bars'));
      }
      const timer = document.createElement('span');
      timer.className = 'voice-live-timer';
      timer.textContent = formatElapsed(Date.now() - recordStartedAt);
      right.appendChild(timer);

      head.append(left, right);
      card.appendChild(head);

      const body = document.createElement('div');
      body.className = 'voice-live-body';
      const hasWords = Boolean(rt.committedText || rt.interimText);
      if (cancelReady) {
        body.dataset.empty = 'true';
        const warn = document.createElement('span');
        warn.className = 'voice-live-placeholder';
        warn.textContent = hasWords
          ? '松开手指将取消录音，并自动撤回刚写入输入框的文字'
          : '松开手指取消本次实时语音输入';
        body.appendChild(warn);
      } else if (!hasWords) {
        body.dataset.empty = 'true';
        const ph = document.createElement('span');
        ph.className = 'voice-live-placeholder';
        ph.textContent = rt.finalizing
          ? '正在校准尾音…'
          : (rt.ready ? '请开始说话，文字将边说边写入输入框…' : '正在建立实时流连接，可直接开始说话…');
        const caret = document.createElement('span');
        caret.className = 'voice-live-caret';
        caret.setAttribute('aria-hidden', 'true');
        body.append(ph, caret);
      } else {
        if (rt.committedText) {
          const committed = document.createElement('span');
          committed.className = 'voice-live-committed';
          committed.textContent = rt.committedText;
          body.appendChild(committed);
        }
        if (rt.interimText) {
          const interim = document.createElement('span');
          interim.className = 'voice-live-interim';
          interim.textContent = rt.interimText;
          body.appendChild(interim);
        }
        const caret = document.createElement('span');
        caret.className = 'voice-live-caret';
        caret.setAttribute('aria-hidden', 'true');
        body.appendChild(caret);
      }
      card.appendChild(body);

      const foot = document.createElement('div');
      foot.className = 'voice-live-foot';
      foot.textContent = rt.finalizing
        ? '正在完成最后词句校准 · 单击话筒可立即结束'
        : (cancelReady ? '向下移回按钮继续听写' : '边说边出字 · 松手完成 · 上滑或按 Esc 取消撤回');
      card.appendChild(foot);

      showPill(card, {tone: cancelReady ? 'cancel' : 'normal', mode: 'realtime'});
      body.scrollTop = body.scrollHeight;
    }

    function snapshotInputSelection() {
      try {
        const start = typeof input.selectionStart === 'number' ? input.selectionStart : input.value.length;
        const end = typeof input.selectionEnd === 'number' ? input.selectionEnd : start;
        return {
          start,
          end,
          before: input.value.slice(0, start),
          after: input.value.slice(end),
          replaced: input.value.slice(start, end),
        };
      } catch {
        const val = String(input.value || '');
        return {start: val.length, end: val.length, before: val, after: '', replaced: ''};
      }
    }

    function applyTranscriptText(sel, nextText) {
      let text = nextText;
      const maxLen = Number(input.maxLength);
      if (Number.isFinite(maxLen) && maxLen > 0) {
        const otherLen = input.value.length - (sel.end - sel.start);
        const budget = Math.max(0, maxLen - otherLen);
        if (text.length > budget) text = text.slice(0, budget);
      }
      const clampedStart = Math.max(0, Math.min(sel.start, input.value.length));
      const clampedEnd = Math.max(clampedStart, Math.min(sel.end, input.value.length));
      if (typeof input.setRangeText === 'function') {
        input.setRangeText(text, clampedStart, clampedEnd, 'end');
      } else {
        input.value = input.value.slice(0, clampedStart) + text + input.value.slice(clampedEnd);
      }
      sel.start = clampedStart;
      sel.end = clampedStart + text.length;
      input.dispatchEvent(new Event('input', {bubbles: true}));
      onInput?.(text, false);
    }

    function rollbackLiveSelection(sel, originalReplaced = '') {
      if (!sel) return;
      const clampedStart = Math.max(0, Math.min(sel.start, input.value.length));
      const clampedEnd = Math.max(clampedStart, Math.min(sel.end, input.value.length));
      if (typeof input.setRangeText === 'function') {
        input.setRangeText(originalReplaced, clampedStart, clampedEnd, 'preserve');
      } else {
        input.value = input.value.slice(0, clampedStart) + originalReplaced + input.value.slice(clampedEnd);
      }
      try {
        input.setSelectionRange(clampedStart, clampedStart + originalReplaced.length);
      } catch {}
      sel.end = clampedStart + originalReplaced.length;
      input.dispatchEvent(new Event('input', {bubbles: true}));
      onInput?.(originalReplaced, false);
    }

    async function openAudioRecorder(onLevel, onPcm16k) {
      if (typeof globalThis.__VOICE_INPUT_TEST_RECORDER__ === 'function') {
        return await globalThis.__VOICE_INPUT_TEST_RECORDER__(onLevel, onPcm16k);
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
      const sampleRate = ctx.sampleRate || 48000;
      const source = ctx.createMediaStreamSource(stream);
      const processor = ctx.createScriptProcessor(2048, 1, 1);
      const gain = ctx.createGain();
      gain.gain.value = 0;
      const chunks = [];
      let pcmCarry = new Int16Array(0);

      processor.onaudioprocess = event => {
        const channel = event.inputBuffer.getChannelData(0);
        const copy = new Float32Array(channel.length);
        copy.set(channel);
        chunks.push(copy);
        let sq = 0;
        for (let i = 0; i < copy.length; i += 4) sq += copy[i] * copy[i];
        const rms = Math.sqrt(sq / Math.max(1, Math.ceil(copy.length / 4)));
        onLevel?.(rms);

        if (typeof onPcm16k === 'function') {
          const down = downsampleFloatToPcm16(copy, sampleRate, TARGET_SAMPLE_RATE);
          if (down.length > 0) {
            const combined = new Int16Array(pcmCarry.length + down.length);
            combined.set(pcmCarry, 0);
            combined.set(down, pcmCarry.length);
            let offset = 0;
            while (offset + PCM_FRAME_SAMPLES <= combined.length) {
              onPcm16k(combined.slice(offset, offset + PCM_FRAME_SAMPLES));
              offset += PCM_FRAME_SAMPLES;
            }
            pcmCarry = combined.slice(offset);
          }
        }
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
          if (typeof onPcm16k === 'function' && pcmCarry.length > 0) {
            onPcm16k(pcmCarry);
            pcmCarry = new Int16Array(0);
          }
          await cleanup();
          return encodeWav16k(chunks, sampleRate);
        },
        async cancel() {
          chunks.length = 0;
          pcmCarry = new Int16Array(0);
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

    function buildContextSnapshot(selSnap) {
      const rawContext = getContext() || {};
      return {
        title: String(rawContext.title || ''),
        messages: Array.isArray(rawContext.messages) ? rawContext.messages : [],
        draft_before: selSnap?.before ?? input.value.slice(0, selSnap?.start ?? input.value.length),
        draft_after: selSnap?.after ?? input.value.slice(selSnap?.end ?? input.value.length),
      };
    }

    function closeRealtimeWs(rt, sendCancel = false) {
      if (!rt) return;
      clearTimeout(rt.finalizeTimer);
      rt.finalizeTimer = null;
      rt.closed = true;
      if (rt.ws) {
        try {
          if (sendCancel && rt.ws.readyState === WebSocket.OPEN) {
            rt.ws.send(JSON.stringify({type: 'cancel'}));
          }
        } catch {}
        try { rt.ws.close(); } catch {}
        rt.ws = null;
      }
    }

    function startRealtimeSession(meta, selSnap) {
      const liveSel = {
        start: Math.min(selSnap.start, input.value.length),
        end: Math.min(selSnap.end, input.value.length),
      };
      const context = buildContextSnapshot(selSnap);
      const extraParams = typeof getRealtimeParams === 'function'
        ? (getRealtimeParams() || {})
        : (getRequestExtra() || {});
      const wsUrl = resolveWsUrl(realtimeEndpoint, {
        ...extraParams,
        model: meta.id,
      });

      const rt = {
        meta,
        liveSel,
        originalReplaced: selSnap.replaced || '',
        committedText: '',
        interimText: '',
        wroteLive: false,
        ready: false,
        finalizing: false,
        closed: false,
        pendingPcm: [],
        ws: null,
        finalizeTimer: null,
      };

      let ws;
      try {
        ws = new WebSocket(wsUrl);
        ws.binaryType = 'arraybuffer';
      } catch (error) {
        rt.closed = true;
        rt.openError = error;
        return rt;
      }
      rt.ws = ws;

      const syncLiveInput = () => {
        if (rt.closed) return;
        const combined = rt.committedText + rt.interimText;
        if (combined || rt.wroteLive) {
          applyTranscriptText(rt.liveSel, combined);
          rt.wroteLive = true;
        }
        renderRealtimeCard(rt, latestAudioLevel);
      };

      const finishRealtimeSuccess = finalText => {
        if (rt.closed) return;
        closeRealtimeWs(rt, false);
        if (realtimeSession === rt) realtimeSession = null;
        if (transcribeAbort === rt.abortAdapter) transcribeAbort = null;

        const cleanFinal = typeof finalText === 'string' ? finalText : (rt.committedText + rt.interimText);
        if (cleanFinal || rt.wroteLive) {
          applyTranscriptText(rt.liveSel, cleanFinal);
          rt.wroteLive = true;
        }

        state = 'idle';
        button.removeAttribute('aria-busy');
        syncButtonMeta();

        if (!cleanFinal.trim()) {
          if (rt.wroteLive) rollbackLiveSelection(rt.liveSel, rt.originalReplaced);
          showPill('未识别到说话内容', {tone: 'cancel', durationMs: 1500});
        } else {
          hidePill();
          try {
            input.focus({preventScroll: true});
            input.setSelectionRange(rt.liveSel.end, rt.liveSel.end);
          } catch {}
          onInput?.(cleanFinal, true);
        }
      };

      const failRealtime = errMsg => {
        if (rt.closed) return;
        closeRealtimeWs(rt, false);
        if (realtimeSession === rt) realtimeSession = null;
        if (transcribeAbort === rt.abortAdapter) transcribeAbort = null;

        if (state === 'recording' && recorderHandle) {
          const h = recorderHandle;
          recorderHandle = null;
          clearRecordingTimers();
          activePointer = null;
          void h.cancel();
        }
        state = 'idle';
        button.removeAttribute('aria-busy');
        syncButtonMeta();
        showPill(`实时语音失败：${errMsg}`, {tone: 'error', durationMs: 3400});
      };

      rt.finishImmediate = () => {
        finishRealtimeSuccess(rt.committedText + rt.interimText);
      };

      rt.abortAdapter = {
        abort() {
          // Clicking the spinner during post-release tail finalization keeps already-streamed words!
          rt.finishImmediate();
        },
      };

      ws.onopen = () => {
        if (rt.closed) return;
        try {
          ws.send(JSON.stringify({
            type: 'start',
            model: meta.id,
            sample_rate: TARGET_SAMPLE_RATE,
            context,
          }));
          for (const chunk of rt.pendingPcm) {
            ws.send(chunk);
          }
          rt.pendingPcm.length = 0;
        } catch (err) {
          failRealtime(err?.message || '连接发送失败');
        }
      };

      ws.onmessage = event => {
        if (rt.closed || typeof event.data !== 'string') return;
        let msg;
        try { msg = JSON.parse(event.data); } catch { return; }
        switch (msg.type) {
          case 'ready':
            rt.ready = true;
            renderRealtimeCard(rt, latestAudioLevel);
            break;
          case 'delta':
            if (typeof msg.delta === 'string' && msg.delta) {
              rt.committedText += msg.delta;
              syncLiveInput();
            }
            break;
          case 'interim':
            if (typeof msg.text === 'string') {
              rt.interimText = msg.text;
              syncLiveInput();
            }
            break;
          case 'segment':
            rt.interimText = '';
            if (typeof msg.text === 'string' && msg.text) {
              rt.committedText += msg.text;
            }
            syncLiveInput();
            break;
          case 'done':
            finishRealtimeSuccess(typeof msg.text === 'string' ? msg.text : (rt.committedText + rt.interimText));
            break;
          case 'error':
            failRealtime(msg.error || '实时识别服务异常');
            break;
          default:
            break;
        }
      };

      ws.onerror = () => {
        if (rt.closed) return;
        if (rt.finalizing && (rt.committedText || rt.interimText)) {
          rt.finishImmediate();
        } else {
          failRealtime('实时语音 WebSocket 连接中断');
        }
      };

      ws.onclose = () => {
        if (rt.closed) return;
        if (rt.finalizing) {
          rt.finishImmediate();
        } else {
          failRealtime('实时语音连接已关闭');
        }
      };

      rt.pushPcm = pcm16 => {
        if (rt.closed || rt.finalizing || !pcm16?.byteLength) return;
        const buf = pcm16.buffer.slice(pcm16.byteOffset, pcm16.byteOffset + pcm16.byteLength);
        if (ws.readyState === WebSocket.OPEN) {
          try { ws.send(buf); } catch {}
        } else if (ws.readyState === WebSocket.CONNECTING && rt.pendingPcm.length < 220) {
          rt.pendingPcm.push(buf);
        }
      };

      return rt;
    }

    async function startRecording() {
      if (state !== 'idle' || computeDisabled()) return;
      closeMenu();
      savedSelection = snapshotInputSelection();
      const meta = modelMeta(currentModelId(), availableProviders());
      cancelReady = false;
      button.dataset.cancelReady = 'false';
      state = 'recording';
      recordStartedAt = Date.now();
      latestAudioLevel = 0;
      syncButtonMeta();

      let rt = null;
      if (meta.realtime) {
        rt = startRealtimeSession(meta, savedSelection);
        if (rt.openError) {
          state = 'idle';
          activePointer = null;
          syncButtonMeta();
          showPill(`无法建立实时语音连接：${rt.openError?.message || rt.openError}`, {tone: 'error', durationMs: 3200});
          return;
        }
        realtimeSession = rt;
        renderRealtimeCard(rt, 0);
      } else {
        showPill('正在请求麦克风…');
      }

      const promptStart = Date.now();
      let handle;
      try {
        handle = await openAudioRecorder(
          rms => { latestAudioLevel = rms; },
          rt ? pcm16 => rt.pushPcm(pcm16) : null,
        );
      } catch (error) {
        if (rt) {
          closeRealtimeWs(rt, true);
          if (realtimeSession === rt) realtimeSession = null;
        }
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
        if (rt) {
          closeRealtimeWs(rt, true);
          if (realtimeSession === rt) realtimeSession = null;
          if (rt.wroteLive) rollbackLiveSelection(rt.liveSel, rt.originalReplaced);
        }
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
      if (rt) {
        renderRealtimeCard(rt, latestAudioLevel);
        recordTickTimer = setInterval(() => {
          if (state === 'recording' && realtimeSession === rt) renderRealtimeCard(rt, latestAudioLevel);
        }, 90);
      } else {
        renderBatchRecordingPill(0);
        recordTickTimer = setInterval(() => {
          if (state === 'recording') renderBatchRecordingPill(latestAudioLevel);
        }, 90);
      }
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
      const rt = realtimeSession;

      if (!handle) {
        if (rt) {
          closeRealtimeWs(rt, true);
          realtimeSession = null;
          if (rt.wroteLive) rollbackLiveSelection(rt.liveSel, rt.originalReplaced);
        }
        state = 'idle';
        syncButtonMeta();
        hidePill();
        return;
      }

      if (cancelled || wasCancelReady) {
        if (rt) {
          closeRealtimeWs(rt, true);
          realtimeSession = null;
          if (rt.wroteLive) rollbackLiveSelection(rt.liveSel, rt.originalReplaced);
        }
        state = 'idle';
        syncButtonMeta();
        await handle.cancel();
        showPill(rt?.wroteLive ? '已取消并撤回实时语音输入' : '已取消录音', {tone: 'cancel', durationMs: 1250});
        return;
      }

      let encoded;
      try {
        encoded = await handle.stop();
      } catch (error) {
        if (rt) {
          closeRealtimeWs(rt, true);
          realtimeSession = null;
        }
        state = 'idle';
        syncButtonMeta();
        showPill(`录音处理失败：${error?.message || error}`, {tone: 'error', durationMs: 2800});
        return;
      }

      const silentAudio = !encoded.base64 || encoded.durationMs < 180 || (encoded.rms < 0.0008 && encoded.peak < 0.004);

      if (rt) {
        const hasLiveWords = Boolean((rt.committedText + rt.interimText).trim());
        if (silentAudio && !hasLiveWords) {
          closeRealtimeWs(rt, true);
          realtimeSession = null;
          if (rt.wroteLive) rollbackLiveSelection(rt.liveSel, rt.originalReplaced);
          state = 'idle';
          syncButtonMeta();
          showPill('未检测到语音', {tone: 'cancel', durationMs: 1500});
          return;
        }

        rt.finalizing = true;
        state = 'transcribing';
        button.setAttribute('aria-busy', 'true');
        transcribeAbort = rt.abortAdapter;
        syncButtonMeta();
        renderRealtimeCard(rt, 0);

        if (rt.ws && rt.ws.readyState === WebSocket.OPEN) {
          try {
            rt.ws.send(JSON.stringify({type: 'commit'}));
          } catch {
            rt.finishImmediate();
            return;
          }
        } else if (rt.ws && rt.ws.readyState === WebSocket.CONNECTING) {
          rt.pendingPcm.push(JSON.stringify({type: 'commit'}));
        } else {
          rt.finishImmediate();
          return;
        }

        rt.finalizeTimer = setTimeout(() => {
          if (!rt.closed) rt.finishImmediate();
        }, 6500);
        return;
      }

      if (silentAudio) {
        state = 'idle';
        syncButtonMeta();
        showPill('未检测到语音', {tone: 'cancel', durationMs: 1500});
        return;
      }

      await streamTranscription(encoded);
    }

    async function streamTranscription(encoded) {
      const model = currentModelId();
      const meta = modelMeta(model, availableProviders());
      const sel = savedSelection
        ? {start: Math.min(savedSelection.start, input.value.length), end: Math.min(savedSelection.end, input.value.length)}
        : {start: input.value.length, end: input.value.length};
      const context = buildContextSnapshot(savedSelection || sel);
      const extra = getRequestExtra() || {};

      state = 'transcribing';
      button.setAttribute('aria-busy', 'true');
      syncButtonMeta();
      showPill(`正在识别（${meta.short}）… 单击可取消`);

      const reqAbort = new AbortController();
      transcribeAbort = reqAbort;
      let streamed = '';

      try {
        const resolvedUrl = resolveHttpUrl(endpoint);
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

    function cancelActiveWork(rollbackRealtime = true) {
      clearTimeout(holdTimer);
      holdTimer = null;
      if (state === 'recording') {
        void finishRecording(true);
      } else if (state === 'transcribing') {
        if (realtimeSession && rollbackRealtime) {
          const rt = realtimeSession;
          closeRealtimeWs(rt, true);
          realtimeSession = null;
          if (transcribeAbort === rt.abortAdapter) transcribeAbort = null;
          if (rt.wroteLive) rollbackLiveSelection(rt.liveSel, rt.originalReplaced);
          state = 'idle';
          button.removeAttribute('aria-busy');
          syncButtonMeta();
          showPill('已取消并撤回实时语音输入', {tone: 'cancel', durationMs: 1200});
        } else if (transcribeAbort) {
          transcribeAbort.abort();
        }
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
          if (realtimeSession) renderRealtimeCard(realtimeSession, latestAudioLevel);
          else renderBatchRecordingPill(latestAudioLevel);
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
          cancelActiveWork(true);
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
        cancelActiveWork(true);
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
        cancelActiveWork(false);
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
    PROVIDERS,
    MODELS,
    DEFAULT_MODEL,
    validModelId,
    encodeWav16k,
    attach,
  });
})();
