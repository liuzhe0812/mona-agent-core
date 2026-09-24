import React from 'react';
import { createRoot } from 'react-dom/client';
import { XlsxViewer, setWasmSource } from '@extend-ai/react-xlsx';
import { validateOfficeZip } from './zip-guard.mjs';
function SheetTabs({ controller, ready }) {
  React.useEffect(() => { if (controller.tabs.length) ready(); }, [controller.tabs.length, ready]);
  return React.createElement('div', { className: 'sheet-tabs', role: 'tablist', 'aria-label': '工作表' }, controller.tabs.map((tab, index) => React.createElement('button', {
    key: tab.id || index, type: 'button', role: 'tab', tabIndex: controller.activeTabIndex === index ? 0 : -1, 'aria-selected': controller.activeTabIndex === index, onClick: () => controller.setActiveTabIndex(index),
    onKeyDown: event => { const count = controller.tabs.length; const next = event.key === 'Home' ? 0 : event.key === 'End' ? count - 1 : event.key === 'ArrowLeft' ? (index + count - 1) % count : event.key === 'ArrowRight' ? (index + 1) % count : -1; if (next >= 0) { event.preventDefault(); controller.setActiveTabIndex(next); event.currentTarget.parentElement.children[next]?.focus(); } },
  }, tab.name || `Sheet ${index + 1}`)));
}
window.MonaOffice = {
  async render(buffer, root, options) {
    await validateOfficeZip(buffer); setWasmSource(options.wasm);
    const react = createRoot(root);
    await new Promise((resolve, reject) => {
      react.render(React.createElement(XlsxViewer, { file: buffer, fileName: options.name, height: '100%', readOnly: true, rounded: false, showDefaultToolbar: false,
        toolbar: controller => React.createElement(SheetTabs, { controller, ready: resolve }), useWorker: false, maxFileSizeBytes: 8 * 1024 * 1024,
        isDark: options.dark, loadingState: React.createElement('p', null, '正在读取工作表…'), errorState: error => { queueMicrotask(() => reject(error)); return React.createElement('p', { role: 'alert' }, '工作表解析失败：' + error.message); } }));
    });
    return () => react.unmount();
  },
};
