import { PptxViewer, RECOMMENDED_ZIP_LIMITS } from '@aiden0z/pptx-renderer/browser';
import { validateOfficeZip } from './zip-guard.mjs';
window.MonaOffice = {
  async render(buffer, root) {
    await validateOfficeZip(buffer);
    const viewer = await PptxViewer.open(buffer, root, { zipLimits: RECOMMENDED_ZIP_LIMITS, lazySlides: true, lazyMedia: true, pdfjs: false, listOptions: { windowed: true, initialSlides: 3, batchSize: 3 } });
    return () => viewer.dispose?.();
  },
};
