'use strict';

(() => {
  const theme = () => document.documentElement.dataset.theme === 'dark' ? 'dark' : 'neutral';
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
  new MutationObserver(() => {
    for (const diagram of document.querySelectorAll('sd-mermaid')) diagram.schedule();
  }).observe(document.documentElement, {attributeFilter: ['data-theme']});

  customElements.define('sd-mermaid', class extends HTMLElement {
    connectedCallback() { observer.observe(this); }

    disconnectedCallback() {
      observer.unobserve(this);
      this.visible = false;
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
            theme: nextTheme,
            flowchart: {useMaxWidth: false, wrappingWidth: 260},
          });
          const {svg: frame} = await mermaid.render(`sd-diagram-${++sequence}`, this.querySelector('code').textContent);
          // Mermaid returns a sandboxed iframe. Display only its SVG as an
          // image so the finished diagram has no active content or links.
          const template = document.createElement('template');
          template.innerHTML = frame;
          const encoded = template.content.querySelector('iframe').getAttribute('src').split(',')[1];
          const html = new TextDecoder().decode(Uint8Array.from(atob(encoded), ch => ch.charCodeAt(0)));
          template.innerHTML = html;
          const svg = template.content.querySelector('svg');
          const image = new Image();
          image.alt = svg.querySelector('title')?.textContent || 'Mermaid 图表';
          const xml = new XMLSerializer().serializeToString(svg);
          image.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(xml)}`;
          await image.decode();
          if (this.isConnected && theme() === nextTheme) {
            this.show(image);
            this.renderedTheme = nextTheme;
          }
        } catch {
          this.renderedTheme = nextTheme;
          // Network and parse errors leave the original code visible.
        } finally {
          this.pending = false;
          if (theme() !== nextTheme) this.schedule();
        }
      });
    }

    show(image) {
      let preview = this.querySelector('.mermaid-preview');
      if (!preview) {
        preview = document.createElement('div');
        preview.className = 'mermaid-preview';
        const button = document.createElement('button');
        button.type = 'button';
        button.className = 'more mermaid-toggle';
        button.textContent = '查看 Mermaid 源码';
        button.onclick = () => {
          const source = this.toggleAttribute('data-source');
          button.textContent = source ? '查看 Mermaid 图形' : '查看 Mermaid 源码';
        };
        this.prepend(button, preview);
      }
      preview.replaceChildren(image);
      this.setAttribute('data-rendered', '');
    }
  });
})();
