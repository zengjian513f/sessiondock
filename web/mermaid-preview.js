'use strict';

(() => {
  const theme = () => document.documentElement.dataset.theme;
  const minZoom = .1, maxZoom = 3, zoomStep = 1.35, readingZoom = .9;
  const zoomControls = `
    <div class="mermaid-scale">
      <button type="button" data-out aria-label="缩小图表" title="缩小图表">−</button>
      <button type="button" data-reset aria-label="恢复阅读大小" title="恢复阅读大小"></button>
      <button type="button" data-in aria-label="放大图表" title="放大图表">+</button>
    </div>
    <button type="button" data-fit>整图</button>`;
  const canvas = `
    <div class="mermaid-preview" tabindex="0" role="region" aria-label="Mermaid 图表">
      <div class="mermaid-board"></div>
    </div>`;
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
    for (const {target} of entries) target.diagramViewport?.layout();
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
      fontSize: '14px',
      background: color('--panel-2'),
      primaryColor: color('--sel'),
      primaryTextColor: color('--text'),
      primaryBorderColor: darkMode ? '#687da0' : '#8399ba',
      lineColor: darkMode ? '#adbed7' : '#516887',
      defaultLinkColor: darkMode ? '#adbed7' : '#516887',
      clusterBkg: color('--panel'),
      clusterBorder: darkMode ? '#56647b' : '#a8b7cf',
      edgeLabelBackground: color('--panel-2'),
      titleColor: color('--text'),
    };
  }

  class DiagramViewport {
    constructor(stage, controls, startAtFirstNode = false) {
      this.stage = stage;
      this.startAtFirstNode = startAtFirstNode;
      this.preview = stage.querySelector('.mermaid-preview');
      this.resetButton = controls.querySelector('[data-reset]');
      this.outButton = controls.querySelector('[data-out]');
      this.inButton = controls.querySelector('[data-in]');
      const fitButton = controls.querySelector('[data-fit]');
      this.resetButton.onclick = () => { this.zoom = null; this.layout(); this.goToStart(); };
      this.outButton.onclick = () => this.resize(this.scale / zoomStep);
      this.inButton.onclick = () => this.resize(this.scale * zoomStep);
      fitButton.onclick = () => this.resize('fit');
      this.zoom = null;
      this.stage.diagramViewport = this;
    }

    show(diagram) {
      const firstRender = !this.size;
      this.size = diagram;
      if (!this.frame) {
        this.frame = document.createElement('iframe');
        this.frame.setAttribute('sandbox', '');
        this.frame.referrerPolicy = 'no-referrer';
        this.preview.querySelector('.mermaid-board').append(this.frame);
      }
      this.frame.title = diagram.title;
      this.frame.srcdoc = diagram.html;
      this.layout();
      if (firstRender) this.goToStart();
    }

    goToStart() {
      this.preview.scrollLeft = 0;
      // In a narrow viewport the first branch of a wide diagram can be below
      // empty space reserved for later nodes. Bring the entry into view.
      this.preview.scrollTop = this.startAtFirstNode && this.preview.scrollWidth > this.preview.clientWidth
        ? Math.max(0, this.size.startY * this.scale - this.preview.clientHeight * .4) : 0;
    }

    layout() {
      if (!this.size || !this.stage.clientWidth || !this.stage.clientHeight) return;
      const {width, height} = this.size;
      const fitWidth = (this.stage.clientWidth - 8) / width;
      this.scale = this.zoom === 'fit'
        ? Math.min(1, fitWidth, (this.stage.clientHeight - 8) / height)
        : this.zoom ?? Math.max(readingZoom, Math.min(1, fitWidth));
      this.frame.style.width = `${Math.ceil(width * this.scale)}px`;
      this.frame.style.height = `${Math.ceil(height * this.scale)}px`;
      this.resetButton.textContent = `${Math.round(this.scale * 100)}%`;
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
      this.zoom = scale === 'fit' ? scale : Math.max(minZoom, Math.min(maxZoom, scale));
      this.layout();
      const next = this.frame.getBoundingClientRect();
      preview.scrollLeft += centerX * this.scale + (next.left - viewport.left) / cssZoom - preview.clientWidth / 2;
      preview.scrollTop += centerY * this.scale + (next.top - viewport.top) / cssZoom - preview.clientHeight / 2;
    }
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
      this.viewer?.stage.closest('dialog').close();
      this.visible = false;
    }

    build() {
      const source = this.firstElementChild;
      this.innerHTML = `
        <div class="mermaid-toolbar">
          <span class="mermaid-status" role="status">正在绘制图表…</span>
          <div class="mermaid-zoom">${zoomControls}
            <button type="button" class="mermaid-expand">放大查看</button>
          </div>
          <button type="button" class="mermaid-toggle">
            <span data-source>源码</span><span data-diagram>图形</span>
          </button>
        </div>
        <div class="mermaid-stage">${canvas}</div>`;
      this.stage = this.querySelector('.mermaid-stage');
      this.stage.append(source);
      this.viewport = new DiagramViewport(this.stage, this.querySelector('.mermaid-zoom'),
        this.hasAttribute('data-left-to-right'));
      this.querySelector('.mermaid-expand').onclick = () => this.openViewer();
      this.querySelector('.mermaid-toggle').onclick = () => {
        const diagram = getComputedStyle(this.viewport.preview).display !== 'none';
        const next = diagram ? 'source' : 'diagram';
        const automatic = this.querySelector('mark') ? 'source' : 'diagram';
        if (next === automatic) delete this.dataset.view;
        else this.dataset.view = next;
        if (next === 'diagram') this.viewport.layout();
      };
    }

    openViewer() {
      const dialog = document.createElement('dialog');
      dialog.className = 'app-dialog mermaid-dialog';
      dialog.setAttribute('aria-label', this.viewport.size.title);
      dialog.innerHTML = `
        <div class="mermaid-toolbar">
          <div class="mermaid-zoom">${zoomControls}</div>
          <button type="button" class="transfer-close" aria-label="关闭图表">×</button>
        </div>
        <div class="mermaid-stage">${canvas}</div>`;
      const stage = dialog.querySelector('.mermaid-stage');
      const viewer = this.viewer = new DiagramViewport(stage, dialog.querySelector('.mermaid-zoom'));
      const close = dialog.querySelector('.transfer-close');
      close.onclick = () => dialog.close();
      dialog.addEventListener('close', () => {
        sizes.unobserve(stage);
        dialog.remove();
        if (this.viewer === viewer) this.viewer = null;
      }, {once: true});
      document.body.append(dialog);
      dialog.showModal();
      viewer.show(this.viewport.size);
      sizes.observe(stage);
      close.focus();
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
            // Apply wrapping before measuring nodes so the rendered HTML and
            // Mermaid's foreignObject geometry agree for long CJK labels.
            themeCSS: `
              .node foreignObject > div {
                display: inline-block !important;
                white-space: normal !important;
                overflow-wrap: anywhere;
                vertical-align: top;
              }
              .flowchart-link { stroke-width: 1.5px; }
              .cluster-label { font-weight: 600; }`,
            flowchart: {useMaxWidth: false, look: 'classic', diagramPadding: 4,
              padding: 8, nodeSpacing: 20, rankSpacing: 28, wrappingWidth: 264},
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
      const [, top, width, height] = svg.getAttribute('viewBox').trim().split(/[\s,]+/).map(Number);
      if (!(width > 0 && height > 0 && Number.isFinite(width + height))) throw new Error('Invalid diagram viewBox');
      const title = svg.querySelector('title')?.textContent || 'Mermaid 图表';
      const scheme = currentTheme === 'dark' ? 'dark' : 'light';
      const background = getComputedStyle(this).backgroundColor;
      const html = `<!doctype html><meta charset="utf-8">
        <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; form-action 'none'">
        <style>
          html { color-scheme: ${scheme}; background: ${background}; }
          html,body { margin: 0; overflow: hidden; width: 100%; height: 100%; }
          body > svg { display: block; width: 100% !important; height: 100% !important; max-width: none !important; }
          svg text, foreignObject { user-select: text; }
        </style>${new XMLSerializer().serializeToString(svg)}`;
      let first = null;
      if (this.hasAttribute('data-left-to-right')) {
        for (const node of svg.querySelectorAll('g.node[transform]')) {
          const transforms = node.transform.baseVal;
          if (!transforms.numberOfItems) continue;
          const {e, f} = transforms.getItem(0).matrix;
          if (!first || e < first.e || (e === first.e && f < first.f)) first = {e, f};
        }
      }
      const diagram = {width, height, title, html, startY: first ? first.f - top : 0};
      this.setAttribute('data-ready', '');
      this.viewport.show(diagram);
      this.viewer?.show(diagram);
    }
  });
})();
