import { renderAsync } from 'docx-preview';
import { validateOfficeZip } from './zip-guard.mjs';
window.MonaOffice = {
  async render(buffer, root) {
    await validateOfficeZip(buffer);
    await renderAsync(buffer, root, root, { breakPages: true, ignoreLastRenderedPageBreak: true, renderAltChunks: false, renderChanges: false, renderComments: false, renderHeaders: true, renderFooters: true, renderFootnotes: true, renderEndnotes: true, useBase64URL: true });
    const fit = () => {
      const first = root.querySelector('section.docx'); if (!first) return;
      const natural = first.offsetWidth, available = root.clientWidth - 24;
      root.style.setProperty('--document-scale', String(Math.min(1, Math.max(0.2, available / natural))));
    };
    fit(); const observer = new ResizeObserver(fit); observer.observe(root);
    return () => observer.disconnect();
  },
};
