'use strict';

(() => {
  const theme = () => document.documentElement.dataset.theme;
  const minZoom = .1, maxZoom = 3, zoomStep = 1.35;
  let sequence = 0;
  // Mermaid configuration is shared. Serialize rendering and skip diagrams
  // that left the viewport while they were queued.
  let queue = Promise.resolve();
  const observer = new IntersectionObserver(entries => {
    for (const {target, isIntersecting} of entries) {
      target.visible = isIntersecting;
      if (isIntersecting) target.schedule();
    }
  });
  const sizes = new ResizeObserver(entries => {
    for (const {target} of entries) target.parentElement?.layout();
  });
  new MutationObserver(() => {
    for (const diagram of document.querySelectorAll('sd-mermaid')) diagram.schedule();
  }).observe(document.documentElement, {attributeFilter: ['data-theme']});

  function themeVariables(darkMode) {
    const css = getComputedStyle(document.documentElement);
    const color = name => css.getPropertyValue(name).trim();
    return {
      darkMode,
      fontFamily: color('--sans'),
      fontSize: '13px',
      background: color('--panel-2'),
      primaryColor: color('--sel'),
      primaryTextColor: color('--text'),
      primaryBorderColor: color('--muted'),
      lineColor: color('--muted'),
      clusterBkg: color('--panel'),
      clusterBorder: color('--border'),
      edgeLabelBackground: color('--panel-2'),
      titleColor: color('--text'),
    };
  }

  function diagramSvg(frame) {
    const template = document.createElement('template');
    template.innerHTML = frame;
    const encoded = template.content.querySelector('iframe').getAttribute('src').split(',')[1];
    template.innerHTML = new TextDecoder().decode(Uint8Array.from(atob(encoded), ch => ch.charCodeAt(0)));
    const svg = template.content.querySelector('svg');
    // Keep label text from source-authored links without handing navigation to the diagram.
    for (const anchor of svg.querySelectorAll('a')) {
      anchor.removeAttribute('href');
      anchor.removeAttribute('xlink:href');
    }
    for (const node of svg.querySelectorAll('meta, base, link')) node.remove();
    return svg;
  }

  async function render(mermaid, source) {
    // Mermaid measures its output in a temporary frame. Keep it outside the
    // app's layout and remove it even when the diagram cannot be parsed.
    const measure = document.createElement('div');
    measure.style.cssText = 'position:fixed; right:100%; width:1000px; height:800px; visibility:hidden';
    document.body.append(measure);
    try {
      const {svg} = await mermaid.render(`sd-diagram-${++sequence}`, source, measure);
      return diagramSvg(svg);
    } finally {
      measure.remove();
    }
  }

  customElements.define('sd-mermaid', class extends HTMLElement {
    connectedCallback() {
      if (!this.stage) this.build();
      sizes.observe(this.stage);
      observer.observe(this);
    }

    disconnectedCallback() {
      observer.unobserve(this);
      sizes.unobserve(this.stage);
      this.visible = false;
    }

    build() {
      const source = this.firstElementChild;
      this.innerHTML = `
        <div class="mermaid-toolbar">
          <span class="mermaid-status" role="status">正在绘制图表…</span>
          <div class="mermaid-zoom">
            <button type="button" data-out aria-label="缩小图表" title="缩小图表">−</button>
            <button type="button" data-fit aria-label="适应窗口" title="适应窗口"></button>
            <button type="button" data-in aria-label="放大图表" title="放大图表">+</button>
          </div>
          <button type="button" class="mermaid-toggle">
            <span data-source>源码</span><span data-diagram>图形</span>
          </button>
        </div>
        <div class="mermaid-stage">
          <div class="mermaid-preview" tabindex="0" role="region" aria-label="Mermaid 图表">
            <div class="mermaid-board"></div>
          </div>
        </div>`;
      this.stage = this.querySelector('.mermaid-stage');
      this.stage.append(source);
      this.preview = this.querySelector('.mermaid-preview');
      this.fitButton = this.querySelector('[data-fit]');
      this.outButton = this.querySelector('[data-out]');
      this.inButton = this.querySelector('[data-in]');
      this.fitButton.onclick = () => this.resize(null);
      this.outButton.onclick = () => this.resize(this.scale / zoomStep);
      this.inButton.onclick = () => this.resize(this.scale * zoomStep);
      this.querySelector('.mermaid-toggle').onclick = () => {
        const diagram = getComputedStyle(this.preview).display !== 'none';
        const next = diagram ? 'source' : 'diagram';
        const automatic = this.querySelector('mark') ? 'source' : 'diagram';
        if (next === automatic) delete this.dataset.view;
        else this.dataset.view = next;
      };
    }

    schedule() {
      const nextTheme = theme();
      if (!this.visible || this.pending || this.renderedTheme === nextTheme) return;
      this.pending = true;
      queue = queue.then(async () => {
        try {
          if (!this.isConnected || !this.visible || theme() !== nextTheme) return;
          await SessionDockAssets.script('vendor/mermaid/mermaid.tiny.js');
          if (!this.isConnected || !this.visible || theme() !== nextTheme) return;
          const mermaid = globalThis.mermaid;
          mermaid.initialize({
            startOnLoad: false,
            securityLevel: 'sandbox',
            suppressErrorRendering: true,
            theme: 'base',
            look: 'classic',
            themeVariables: themeVariables(nextTheme === 'dark'),
            flowchart: {useMaxWidth: false, look: 'classic', diagramPadding: 4,
              padding: 6, nodeSpacing: 18, rankSpacing: 22, wrappingWidth: 212},
          });
          const svg = await render(mermaid, this.querySelector('code').textContent);
          if (this.isConnected && theme() === nextTheme) {
            this.show(svg, nextTheme);
            this.renderedTheme = nextTheme;
          }
        } catch {
          this.renderedTheme = nextTheme;
          this.querySelector('.mermaid-status').textContent = 'Mermaid 源码';
        } finally {
          this.pending = false;
          if (theme() !== nextTheme) this.schedule();
        }
      });
    }

    show(svg, currentTheme) {
      const [,, width, height] = svg.getAttribute('viewBox').trim().split(/[\s,]+/).map(Number);
      if (!(width > 0 && height > 0 && Number.isFinite(width + height))) throw new Error('Invalid diagram viewBox');
      this.diagramSize = {width, height};
      if (!this.frame) {
        this.frame = document.createElement('iframe');
        this.frame.setAttribute('sandbox', '');
        this.frame.referrerPolicy = 'no-referrer';
        this.querySelector('.mermaid-board').append(this.frame);
      }
      this.frame.title = svg.querySelector('title')?.textContent || 'Mermaid 图表';
      const scheme = currentTheme === 'dark' ? 'dark' : 'light';
      const background = getComputedStyle(this).backgroundColor;
      this.frame.srcdoc = `<!doctype html><meta charset="utf-8">
        <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; form-action 'none'">
        <style>
          html { color-scheme: ${scheme}; background: ${background}; }
          html,body { margin: 0; overflow: hidden; width: 100%; height: 100%; }
          body > svg { display: block; width: 100% !important; height: 100% !important; max-width: none !important; }
          svg text, foreignObject { user-select: text; }
        </style>${new XMLSerializer().serializeToString(svg)}`;
      this.layout();
      this.setAttribute('data-ready', '');
    }

    fittingScale() {
      const {width, height} = this.diagramSize;
      return Math.min(1, (this.stage.clientWidth - 8) / width, (this.stage.clientHeight - 8) / height);
    }

    layout() {
      if (!this.diagramSize || !this.stage.clientWidth || !this.stage.clientHeight) return;
      const {width, height} = this.diagramSize;
      this.scale = this.zoom ?? this.fittingScale();
      this.frame.style.width = `${Math.ceil(width * this.scale)}px`;
      this.frame.style.height = `${Math.ceil(height * this.scale)}px`;
      this.fitButton.textContent = `${Math.round(this.scale * 100)}%`;
      this.outButton.disabled = this.scale <= minZoom;
      this.inButton.disabled = this.scale >= maxZoom;
    }

    resize(scale) {
      const preview = this.preview;
      const bounds = this.frame.getBoundingClientRect();
      const viewport = preview.getBoundingClientRect();
      const cssZoom = preview.currentCSSZoom || 1;
      const centerX = (preview.clientWidth / 2 + (viewport.left - bounds.left) / cssZoom) / this.scale;
      const centerY = (preview.clientHeight / 2 + (viewport.top - bounds.top) / cssZoom) / this.scale;
      this.zoom = scale === null ? null : Math.max(minZoom, Math.min(maxZoom, scale));
      this.layout();
      const next = this.frame.getBoundingClientRect();
      preview.scrollLeft += centerX * this.scale + (next.left - viewport.left) / cssZoom - preview.clientWidth / 2;
      preview.scrollTop += centerY * this.scale + (next.top - viewport.top) / cssZoom - preview.clientHeight / 2;
    }
  });
})();
