

import * as Overlays from '../../migration/overlays'

export function createFormulaRuntime() {
let formulaLoading = null;

const formulaRoots = new Set();

function renderFormulae(root) {
  if (!/\$|\\[([]/.test(root.textContent || '')) return;
  if (typeof window.renderMathInElement !== 'function') {
    formulaRoots.add(root);
    if (!formulaLoading) {
      formulaLoading = Promise.all([
        Overlays.assets.style('vendor/katex/katex.min.css'),
        Overlays.assets.script('vendor/katex/katex.min.js'),
      ]).then(() => Overlays.assets.script('vendor/katex/auto-render.min.js'))
        .then(() => {
          const roots = [...formulaRoots];
          formulaRoots.clear();
          for (const pending of roots) {
            if (pending.isConnected || pending.getRootNode().nodeType === Node.DOCUMENT_FRAGMENT_NODE)
              renderFormulae(pending);
          }
        }).catch(() => { formulaRoots.clear(); })
        .finally(() => { formulaLoading = null; });
    }
    return;
  }
  try {
    window.renderMathInElement(root, {
      delimiters: [
        { left: '$$', right: '$$', display: true },
        { left: '\\[', right: '\\]', display: true },
        { left: '\\(', right: '\\)', display: false },
        { left: '$', right: '$', display: false },
      ],
      ignoredTags: ['script', 'noscript', 'style', 'textarea', 'pre', 'code'],
      throwOnError: false,
      strict: 'ignore',
      trust: false,
    });
  } catch { /* 单个坏公式按原文保留，不能拖垮整条消息 */ }
}
return {get formulaLoading(){return formulaLoading},formulaRoots,renderFormulae};
}
